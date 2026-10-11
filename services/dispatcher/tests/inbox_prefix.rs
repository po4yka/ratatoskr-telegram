//! The dispatcher's notification connection against a broker that enforces its own NATS stanza
//! (XR-021 CONTRACTS.md section R3-01).
//!
//! The broker is a real `nats-server` whose only service user is the stanza of
//! `deploy/nats/identity.conf`, so a reply subject outside `_INBOX_telegram.>` is never delivered
//! to the dispatcher. `nats-server` must be on `PATH`; a missing binary fails the test.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::io::Write as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use async_nats::jetstream;
use telegram_core::RuntimeRole;

/// The private port range XR-021 reserves for local servers.
const PORTS: std::ops::RangeInclusive<u16> = 57850..=57859;
const PLACEHOLDER: &str = "UREPLACE_ME_WITH_THE_PUBLIC_NKEY_OF_RATATOSKR_TELEGRAM_XXXX";

/// Kills the broker when the test ends, however it ends.
struct Broker {
    child: Child,
    url: String,
    _store: tempfile::TempDir,
}

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    PORTS
        .clone()
        .find(|port| TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .expect("a free port in the private range")
}

fn identity_stanza(public_key: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("deploy/nats/identity.conf");
    let stanza = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert!(
        stanza.contains(PLACEHOLDER),
        "the Telegram placeholder is absent"
    );
    stanza.replace(PLACEHOLDER, public_key)
}

/// Starts `nats-server` with the Telegram stanza as its only service user, plus an administrator
/// standing in for Edge, which provisions the stream and the durable.
fn start_broker(telegram_public: &str, admin_public: &str) -> Broker {
    let store = tempfile::tempdir().expect("JetStream store");
    let port = free_port();
    let config = format!(
        "listen: 127.0.0.1:{port}\n\
         jetstream {{ store_dir: \"{}\" }}\n\
         authorization {{\n  users = [\n{}\n    {{\n      nkey: {admin_public}\n      permissions: {{\n        \
         publish: {{ allow: [\"$JS.API.>\", \"$JS.ACK.>\", \"evt.>\"] }}\n        \
         subscribe: {{ allow: [\"_INBOX.>\"] }}\n      }}\n    }}\n  ]\n}}\n",
        store.path().display(),
        identity_stanza(telegram_public),
    );
    let config_path = store.path().join("broker.conf");
    std::fs::write(&config_path, config).expect("broker configuration");
    let child = Command::new("nats-server")
        .arg("-c")
        .arg(&config_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("nats-server must be on PATH");
    Broker {
        child,
        url: format!("nats://127.0.0.1:{port}"),
        _store: store,
    }
}

async fn connect_admin(url: &str, seed: &str) -> async_nats::Client {
    for _ in 0..100 {
        if let Ok(client) = async_nats::ConnectOptions::with_nkey(seed.to_owned())
            .connect(url)
            .await
        {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the broker never accepted the administrator");
}

fn write_seed(directory: &Path, name: &str, seed: &str) -> PathBuf {
    let path = directory.join(name);
    let mut file = std::fs::File::create(&path).expect("seed file");
    writeln!(file, "{seed}").expect("write seed");
    path
}

#[tokio::test]
async fn the_notification_consumer_completes_jetstream_requests_under_its_inbox_prefix() {
    let telegram = nkeys::KeyPair::new_user();
    let admin = nkeys::KeyPair::new_user();
    let admin_seed = admin.seed().expect("admin seed");
    let broker = start_broker(&telegram.public_key(), &admin.public_key());
    let seeds = tempfile::tempdir().expect("seed directory");
    let telegram_seed = write_seed(
        seeds.path(),
        "telegram.nkey",
        &telegram.seed().expect("telegram seed"),
    );

    let mut config =
        telegram_core::TelegramConfig::defaults(RuntimeRole::Dispatcher).notification_bus;
    config.endpoint = broker.url.parse().expect("broker URL");
    config.credentials_file = Some(telegram_seed);

    // Edge's part: create the stream and the fixed durable, then publish one fact.
    let admin_client = connect_admin(&broker.url, &admin_seed).await;
    let context = jetstream::new(admin_client);
    let stream = context
        .create_stream(jetstream::stream::Config {
            name: config.stream.clone(),
            subjects: vec!["evt.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("fixture stream");
    stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some(config.durable.clone()),
            filter_subject: config.subject.clone(),
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ack_wait: Duration::from_secs(config.ack_wait_seconds),
            ..jetstream::consumer::pull::Config::default()
        })
        .await
        .expect("fixture durable");
    context
        .publish(config.subject.clone(), "one fact".into())
        .await
        .expect("publish")
        .await
        .expect("stream acknowledgment");

    // The dispatcher's part, through its real connection function.
    let consumer = tokio::time::timeout(
        Duration::from_secs(20),
        ratatoskr_telegram_dispatcher::notifications::open_preprovisioned_consumer(&config),
    )
    .await
    .expect("the consumer lookup must not hang")
    .expect("the pre-provisioned consumer under the Telegram stanza");

    let mut batch = consumer
        .fetch()
        .max_messages(1)
        .expires(Duration::from_secs(5))
        .messages()
        .await
        .expect("fetch under the Telegram stanza");
    let message = futures_util::StreamExt::next(&mut batch)
        .await
        .expect("one fetched message")
        .expect("the delivery");
    assert_eq!(message.payload.as_ref(), b"one fact");
    message.double_ack().await.expect("acknowledged");

    let mut fixture = stream
        .get_consumer::<jetstream::consumer::pull::Config>(&config.durable)
        .await
        .expect("durable");
    let info = fixture.info().await.expect("consumer info");
    assert_eq!(
        info.ack_floor.stream_sequence, 1,
        "the message was acknowledged"
    );
}
