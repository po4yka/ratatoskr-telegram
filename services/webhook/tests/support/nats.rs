//! Task-namespaced authenticated NATS fixture shared by process boot tests.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// A real NATS server with the production least-privilege Telegram user shape.
pub(crate) struct NatsFixture {
    container: String,
    directory: tempfile::TempDir,
    pub(crate) url: String,
    telegram_seed: PathBuf,
}

impl NatsFixture {
    pub(crate) fn start() -> Self {
        use nkeys::KeyPair;

        let suffix = uuid::Uuid::now_v7().simple().to_string();
        let container = format!("ratatoskr-telegram-boot-nats-{suffix}");
        let directory = tempfile::Builder::new()
            .prefix(".nats-fixture-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("NATS fixture directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))
                .expect("NATS fixture directory permissions");
        }
        let admin = KeyPair::new_user();
        let telegram = KeyPair::new_user();
        let config = nats_config(&admin, &telegram);
        std::fs::write(directory.path().join("nats.conf"), config).expect("NATS config");
        let telegram_seed = directory.path().join("telegram.nkey");
        std::fs::write(
            &telegram_seed,
            format!("{}\n", telegram.seed().expect("Telegram seed")),
        )
        .expect("Telegram seed file");
        let mount_source = std::fs::canonicalize(directory.path()).expect("canonical NATS fixture");
        let mount = format!("{}:/etc/nats-fixture:ro", mount_source.display());
        start_container(&container, &mount);
        let url = mapped_url(&container);
        let fixture = Self {
            container,
            directory,
            url,
            telegram_seed,
        };
        fixture.provision(admin.seed().expect("admin seed"));
        fixture
    }

    fn provision(&self, admin_seed: String) {
        let runtime = tokio::runtime::Runtime::new().expect("NATS setup runtime");
        runtime.block_on(async {
            let client = connect_admin(&self.url, admin_seed).await;
            let context = async_nats::jetstream::new(client);
            let stream = context
                .create_stream(async_nats::jetstream::stream::Config {
                    name: "ratatoskr_events".to_owned(),
                    subjects: vec!["evt.>".to_owned()],
                    ..async_nats::jetstream::stream::Config::default()
                })
                .await
                .expect("event stream");
            stream
                .create_consumer(async_nats::jetstream::consumer::pull::Config {
                    durable_name: Some("ratatoskr_telegram_notifications".to_owned()),
                    filter_subject: "evt.platform.notification.raised.v1".to_owned(),
                    ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                    ack_wait: Duration::from_secs(30),
                    ..async_nats::jetstream::consumer::pull::Config::default()
                })
                .await
                .expect("fixed consumer");
        });
    }

    pub(crate) fn telegram_seed_path(&self) -> &str {
        self.telegram_seed.to_str().expect("seed path UTF-8")
    }
}

impl Drop for NatsFixture {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.container])
            .output();
        let _ = self.directory.path();
    }
}

fn nats_config(admin: &nkeys::KeyPair, telegram: &nkeys::KeyPair) -> String {
    format!(
        r#"
port: 4222
host: 0.0.0.0
jetstream {{ store_dir: /data }}
authorization {{
  users: [
    {{ nkey: {} }},
    {{
      nkey: {}
      permissions: {{
        publish: {{ allow: [
          "$JS.API.CONSUMER.INFO.ratatoskr_events.ratatoskr_telegram_notifications",
          "$JS.API.CONSUMER.MSG.NEXT.ratatoskr_events.ratatoskr_telegram_notifications",
          "$JS.ACK.ratatoskr_events.ratatoskr_telegram_notifications.>"
        ] }}
        subscribe: {{ allow: ["_INBOX.>"] }}
      }}
    }}
  ]
}}
"#,
        admin.public_key(),
        telegram.public_key(),
    )
}

fn start_container(container: &str, mount: &str) {
    let started = Command::new("docker")
        .args([
            "run",
            "--detach",
            "--name",
            container,
            "--publish",
            "127.0.0.1::4222",
            "--volume",
            mount,
            "nats@sha256:d4ac35882ac65aff236cd65b9d3fa4d24332c681e1a85f94eedccd3cdd65b1da",
            "-c",
            "/etc/nats-fixture/nats.conf",
        ])
        .output()
        .expect("docker starts NATS");
    assert!(
        started.status.success(),
        "NATS start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
}

fn mapped_url(container: &str) -> String {
    let port = Command::new("docker")
        .args(["port", container, "4222/tcp"])
        .output()
        .expect("docker reports NATS port");
    if !port.status.success() {
        let logs = Command::new("docker")
            .args(["logs", container])
            .output()
            .ok()
            .map_or_else(
                || "logs unavailable".to_owned(),
                |output| String::from_utf8_lossy(&output.stderr).into_owned(),
            );
        let _ = Command::new("docker")
            .args(["rm", "--force", container])
            .output();
        panic!("NATS port unavailable: {logs}");
    }
    let binding = String::from_utf8(port.stdout).expect("port UTF-8");
    let port = binding
        .trim()
        .rsplit_once(':')
        .map(|(_, port)| port)
        .expect("mapped port");
    format!("nats://127.0.0.1:{port}")
}

async fn connect_admin(url: &str, seed: String) -> async_nats::Client {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match async_nats::ConnectOptions::with_nkey(seed.clone())
            .connect(url)
            .await
        {
            Ok(client) => return client,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!("NATS admin connection failed: {error}"),
        }
    }
}
