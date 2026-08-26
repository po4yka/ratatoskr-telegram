//! Every deployable boots on the configuration `DEVELOPMENT.md` documents.
//!
//! This is the test that runs the shipped binaries as processes. It exists so that the local-run
//! block of `DEVELOPMENT.md` cannot rot: the admin plane is probed over a real socket, and the
//! documented SIGTERM shutdown is asserted to exit `0`.
//!
//! Since plan item 4 both roles write through `PostgreSQL` and refuse to start without a prepared
//! database; the webhook role additionally demands its bot token and webhook secret, and its boot
//! brings a disposable `PostgreSQL` database and an in-test harness Bot API server — no test
//! contacts Telegram.
//!
//! It lives in `services/webhook` because that is the one package cargo builds both binaries for;
//! `cargo test --workspace` is the documented command and it builds the other one alongside.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::io::{Read, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use sqlx::Row as _;

/// How long a binary may take to answer `/health/ready` with `200`. Generous: a loaded CI runner
/// starting a cold process is the slow case, and the cost of a too-short timeout is a flake.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Between readiness polls.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a correctly-refusing process has to exit on its own. A refusing binary writes its
/// report and exits immediately; the timeout only bounds a regression that stays up.
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(15);

/// Synthetic intake credentials for the webhook role's environment. Never real ones.
const SECRET_TOKEN: &str = "webhook-secret-0123456789abcdef";
const BOT_TOKEN: &str = "123456:TEST-boot-harness-token";

/// The dispatcher writes every projection through `PostgreSQL` since item 4: without a database
/// URL it refuses to start with the `EX_CONFIG` report naming `DATABASE__URL`, never binding
/// anything.
#[test]
fn the_dispatcher_requires_a_database_configuration_to_start() {
    let mut child = Command::new(built_binary("ratatoskr-telegram-dispatcher"))
        .env_remove("RATATOSKR__DATABASE__URL")
        .env("RATATOSKR__TELEMETRY__LOG_FORMAT", "pretty")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary must spawn");

    let refused = wait_for_exit(&mut child, REFUSAL_TIMEOUT);
    // Kill before draining: reading the pipe of a process that is still running blocks forever.
    if refused.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let log = format!(
        "--- stdout ---\n{}--- stderr ---\n{}",
        drain(child.stdout.take()),
        drain(child.stderr.take()),
    );

    let status = refused.expect("the dispatcher must exit on its own when refusing");
    assert_eq!(
        status.code(),
        Some(78),
        "a missing database is EX_CONFIG for this role\n{log}"
    );
    assert!(
        log.contains("DATABASE__URL"),
        "the report must name DATABASE__URL\n{log}",
    );
}

/// A configured but unreachable database is equally a startup refusal since item 4: the
/// dispatcher claims and settles through the pool, so staying up would mean workers that cannot
/// work. Exit `1` — a runtime dependency failure, not a configuration one — and the operator is
/// told the database failed.
#[test]
fn a_dispatcher_with_an_unreachable_database_refuses_to_start() {
    let mut child = Command::new(built_binary("ratatoskr-telegram-dispatcher"))
        .env("RATATOSKR__TELEMETRY__LOG_FORMAT", "pretty")
        // Port 5 on loopback: nothing listens there, so connect fails fast instead of timing out.
        .env(
            "RATATOSKR__DATABASE__URL",
            "postgres://nobody:nope@127.0.0.1:5/nowhere",
        )
        .env("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463")
        .env("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test")
        .env(
            "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary must spawn");

    let refused = wait_for_exit(&mut child, REFUSAL_TIMEOUT);
    if refused.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let log = format!(
        "--- stdout ---\n{}--- stderr ---\n{}",
        drain(child.stdout.take()),
        drain(child.stderr.take()),
    );

    let status = refused.expect("the dispatcher must exit on its own when refusing");
    assert_eq!(
        status.code(),
        Some(1),
        "an unreachable database is a startup refusal for this role\n{log}"
    );
    assert!(
        log.to_lowercase().contains("database"),
        "the operator was not told the dependency failed:\n{log}",
    );
}

/// The webhook role boots once everything it requires is configured: a reachable database, a bot
/// token pointed at a harness Bot API server, and the webhook secret on both sides.
#[test]
fn the_webhook_boots_with_full_intake_configuration_and_reports_ready() {
    const ADMIN_PORT: u16 = 9478;
    const PUBLIC_PORT: u16 = 9479;

    let harness = harness_bot_api();
    let runtime = tokio::runtime::Runtime::new().expect("boot runtime");
    let database_url = runtime.block_on(async {
        let test = telegram_persistence::test_support::TestDatabase::create()
            .await
            .expect("a disposable database");
        test.url()
    });

    let outcome = boots(
        "ratatoskr-telegram-webhook",
        &[
            ("RATATOSKR__TELEMETRY__LOG_FORMAT", "pretty"),
            ("RATATOSKR__ADMIN__BIND", &format!("127.0.0.1:{ADMIN_PORT}")),
            (
                "RATATOSKR__WEBHOOK__BIND",
                &format!("127.0.0.1:{PUBLIC_PORT}"),
            ),
            ("RATATOSKR__DATABASE__URL", database_url.as_str()),
            ("RATATOSKR__BOT_API__BASE_URL", harness.url.as_str()),
            ("RATATOSKR__BOT_API__TOKEN", BOT_TOKEN),
            ("RATATOSKR__WEBHOOK__SECRET_TOKEN", SECRET_TOKEN),
            ("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "700100200"),
            ("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463"),
            ("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test"),
            (
                "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
                "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
            ),
        ],
        ADMIN_PORT,
    );
    assert!(
        outcome.contains("bot identity"),
        "the webhook should log which bot it serves:\n{outcome}",
    );

    // The harness saw exactly one call: getMe at startup. teloxide uses the Bot API's own
    // method-name casing.
    let calls = harness.calls.lock().expect("harness lock");
    assert_eq!(calls.len(), 1, "startup must call getMe exactly once");
    assert!(
        calls[0].eq_ignore_ascii_case("/bot123456:TEST-boot-harness-token/getme"),
        "the call was not getMe: {}",
        calls[0],
    );
}

/// D3 end to end, as real processes: startup seeds exactly one enabled owner row from the
/// configured id, and an operator-disabled row survives a restart unchanged and singular —
/// bootstrap is insert-if-absent, never a resurrection.
#[test]
fn startup_provisions_owner_once_without_resurrection() {
    const ADMIN_PORT: u16 = 9481;
    const PUBLIC_PORT: u16 = 9482;

    let harness = harness_bot_api();
    let runtime = tokio::runtime::Runtime::new().expect("boot runtime");
    let test = runtime
        .block_on(async { telegram_persistence::test_support::TestDatabase::create().await })
        .expect("a disposable database");
    let database_url = test.url();
    let pool = runtime.block_on(async {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("the fixture database connects")
    });

    let env = [
        ("RATATOSKR__TELEMETRY__LOG_FORMAT", "pretty"),
        ("RATATOSKR__ADMIN__BIND", &format!("127.0.0.1:{ADMIN_PORT}")),
        (
            "RATATOSKR__WEBHOOK__BIND",
            &format!("127.0.0.1:{PUBLIC_PORT}"),
        ),
        ("RATATOSKR__DATABASE__URL", database_url.as_str()),
        ("RATATOSKR__BOT_API__BASE_URL", harness.url.as_str()),
        ("RATATOSKR__BOT_API__TOKEN", BOT_TOKEN),
        ("RATATOSKR__WEBHOOK__SECRET_TOKEN", SECRET_TOKEN),
        ("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "700100200"),
        ("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463"),
        ("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test"),
        (
            "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        ),
    ];

    boots("ratatoskr-telegram-webhook", &env, ADMIN_PORT);

    let enabled: i64 = runtime.block_on(async {
        sqlx::query_scalar(
            "select count(*)::bigint from telegram.identities where access_state = 'enabled'",
        )
        .fetch_one(&pool)
        .await
        .expect("the count")
    });
    assert_eq!(
        enabled, 1,
        "a fresh database gains exactly one enabled owner"
    );

    // The documented interim control: direct SQL while the process is down.
    runtime.block_on(async {
        sqlx::query("update telegram.identities set access_state = 'disabled'")
            .execute(&pool)
            .await
            .expect("the disable");
    });

    boots("ratatoskr-telegram-webhook", &env, ADMIN_PORT);

    let states: Vec<String> = runtime.block_on(async {
        sqlx::query("select access_state from telegram.identities")
            .fetch_all(&pool)
            .await
            .expect("the rows")
            .into_iter()
            .map(|row| row.get::<String, _>("access_state"))
            .collect()
    });
    assert_eq!(states.len(), 1, "a restart must not duplicate the owner");
    assert_eq!(
        states.first().map(String::as_str),
        Some("disabled"),
        "a restart must not resurrect a disabled owner"
    );

    runtime.block_on(async {
        pool.close().await;
        test.cleanup().await.expect("cleanup");
    });
}

/// A webhook whose database cannot be reached refuses to start: it writes through the pool, so
/// staying up would mean binding a listener that cannot serve its purpose. Exit `1`, not `0`.
#[test]
fn a_webhook_whose_database_is_unreachable_refuses_to_start() {
    let path = built_binary("ratatoskr-telegram-webhook");
    let refused = Command::new(&path)
        // Port 5 on loopback: nothing listens there, so connect fails fast instead of timing out.
        .env(
            "RATATOSKR__DATABASE__URL",
            "postgres://nobody:nope@127.0.0.1:5/nowhere",
        )
        .env("RATATOSKR__BOT_API__TOKEN", BOT_TOKEN)
        .env("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463")
        .env("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test")
        .env(
            "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        )
        .env("RATATOSKR__WEBHOOK__SECRET_TOKEN", SECRET_TOKEN)
        .env("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "700100200")
        .output()
        .expect("the binary must run");

    let log = format!(
        "{}{}",
        strip_ansi(&String::from_utf8_lossy(&refused.stdout)),
        String::from_utf8_lossy(&refused.stderr),
    );
    assert_eq!(
        refused.status.code(),
        Some(1),
        "an unreachable database is a startup refusal for this role\n{log}",
    );
    assert!(
        log.to_lowercase().contains("database"),
        "the operator was not told the dependency failed:\n{log}",
    );
}

/// `check-config` is the documented init-container and CI pre-flight, so its exit codes are an
/// operational contract: `0` valid, `78` invalid, and the report never quotes a supplied value.
#[test]
fn check_config_exits_zero_when_valid_and_78_when_invalid() {
    // The dispatcher requires its database since item 4, exactly like the webhook's intake set.
    let unconfigured_dispatcher = Command::new(built_binary("ratatoskr-telegram-dispatcher"))
        .arg("check-config")
        .env_remove("RATATOSKR__DATABASE__URL")
        .output()
        .expect("check-config must run");
    let dispatcher_report = String::from_utf8_lossy(&unconfigured_dispatcher.stderr);
    assert_eq!(
        unconfigured_dispatcher.status.code(),
        Some(78),
        "EX_CONFIG\n{dispatcher_report}"
    );
    assert!(
        dispatcher_report.contains("database.url"),
        "the missing database url must be named:\n{dispatcher_report}"
    );

    // And with the database plus the Platform section present, it validates.
    let configured_dispatcher = Command::new(built_binary("ratatoskr-telegram-dispatcher"))
        .arg("check-config")
        .env(
            "RATATOSKR__DATABASE__URL",
            "postgres://telegram@127.0.0.1:5432/telegram",
        )
        .env("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463")
        .env("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test")
        .env(
            "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        )
        .output()
        .expect("check-config must run");
    assert_eq!(
        configured_dispatcher.status.code(),
        Some(0),
        "dispatcher with a database url validates: {}",
        String::from_utf8_lossy(&configured_dispatcher.stderr),
    );

    // The webhook does not: every intake requirement is named, and nothing is echoed.
    let unconfigured = Command::new(built_binary("ratatoskr-telegram-webhook"))
        .arg("check-config")
        .env_remove("RATATOSKR__DATABASE__URL")
        .output()
        .expect("check-config must run");
    let report = String::from_utf8_lossy(&unconfigured.stderr);
    assert_eq!(unconfigured.status.code(), Some(78), "EX_CONFIG\n{report}");
    for required in [
        "bot_api.token",
        "webhook.secret_token",
        "database.url",
        "access.owner_telegram_user_id",
        "platform.audience",
        "platform.assertion_signing_key",
    ] {
        assert!(report.contains(required), "{required} not named:\n{report}");
    }

    // Fully configured, the webhook validates too.
    let configured = Command::new(built_binary("ratatoskr-telegram-webhook"))
        .arg("check-config")
        .env(
            "RATATOSKR__DATABASE__URL",
            "postgres://telegram@127.0.0.1:5432/telegram",
        )
        .env("RATATOSKR__BOT_API__TOKEN", BOT_TOKEN)
        .env("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463")
        .env("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test")
        .env(
            "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        )
        .env("RATATOSKR__WEBHOOK__SECRET_TOKEN", SECRET_TOKEN)
        .env("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "700100200")
        .output()
        .expect("check-config must run");
    let report = String::from_utf8_lossy(&configured.stderr);
    assert_eq!(
        configured.status.code(),
        Some(0),
        "EX_OK expected\n{report}"
    );
    assert!(
        !report.contains(BOT_TOKEN) && !report.contains(SECRET_TOKEN),
        "check-config echoed a secret:\n{report}",
    );

    // And a bad database scheme is still a value-free report.
    let invalid = Command::new(built_binary("ratatoskr-telegram-dispatcher"))
        .arg("check-config")
        .env(
            "RATATOSKR__DATABASE__URL",
            "mysql://user:secret@db.example:3306/x",
        )
        .env("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463")
        .env("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test")
        .env(
            "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        )
        .output()
        .expect("check-config must run");
    let report = String::from_utf8_lossy(&invalid.stderr);
    assert_eq!(invalid.status.code(), Some(78), "EX_CONFIG\n{report}");
    assert!(report.contains("database.url"), "{report}");
    assert!(
        !report.contains("secret") && !report.contains("mysql://"),
        "the report echoed the supplied value: {report}",
    );
}

/// A listener that cannot bind is a runtime startup failure — exit `1`, not `78` and not `0` — and
/// the operator is told which listener failed.
#[test]
fn a_listener_that_cannot_bind_exits_one() {
    // Held open for the child's whole life; a second listener on the same port is `EADDRINUSE`.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("a port must be available");
    let port = taken.local_addr().expect("the port is known").port();

    let refused = Command::new(built_binary("ratatoskr-telegram-dispatcher"))
        .env("RATATOSKR__ADMIN__BIND", format!("127.0.0.1:{port}"))
        // A reachable database, so the process gets past preparation and reaches the bind that
        // this test is about.
        .env(
            "RATATOSKR__DATABASE__URL",
            "postgres://telegram:telegram@127.0.0.1:15437/telegram",
        )
        .env("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463")
        .env("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test")
        .env(
            "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        )
        .output()
        .expect("the binary must run");

    assert_eq!(
        refused.status.code(),
        Some(1),
        "a bind failure is exit 1, not 78 and not 0\n{}{}",
        strip_ansi(&String::from_utf8_lossy(&refused.stdout)),
        String::from_utf8_lossy(&refused.stderr),
    );
    assert!(
        strip_ansi(&String::from_utf8_lossy(&refused.stdout))
            .contains("the admin listener could not bind"),
        "the operator was not told which listener failed",
    );
}

/// An in-process harness Bot API server: answers every request with a recorded `getMe` body and
/// records the paths it was asked for.
struct HarnessBotApi {
    url: String,
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

/// Spawns the harness on an ephemeral port. One OS thread; it lives until the test process does,
/// which is exactly as long as the child needs it.
fn harness_bot_api() -> HarnessBotApi {
    use std::io::BufRead as _;
    use std::sync::{Arc, Mutex};

    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("harness binds");
    let port = listener.local_addr().expect("local addr").port();
    let recorder = Arc::clone(&calls);
    std::thread::spawn(move || {
        let body = include_str!("../../../crates/bot-api/tests/fixtures/get_me.json");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len(),
        );
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let mut writer = std::io::BufWriter::new(&stream);
            let mut reader = std::io::BufReader::new(&stream);
            // One request head, then answer: `Connection: close` ends the conversation.
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
                continue;
            }
            loop {
                let mut header = String::new();
                match reader.read_line(&mut header) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if header == "\r\n" => break,
                    Ok(_) => {}
                }
            }
            if let Some(path) = request_line.split_whitespace().nth(1) {
                recorder.lock().expect("harness lock").push(path.to_owned());
            }
            let _ = writer.write_all(response.as_bytes());
        }
    });
    HarnessBotApi {
        url: format!("http://127.0.0.1:{port}"),
        calls,
    }
}

/// Spawns `binary` with `env`, waits for readiness on `admin_port`, sends SIGTERM, and asserts a
/// clean exit. Both streams are reported with every failure: stdout carries the log records, stderr
/// only what is written before a subscriber exists. Returns stdout for content assertions.
fn boots(binary: &str, env: &[(&str, &str)], admin_port: u16) -> String {
    let path = built_binary(binary);
    let mut child = Command::new(&path)
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("{} could not be spawned: {error}", path.display()));

    let ready = wait_until_ready(admin_port);
    terminate(&mut child);
    let status = child
        .wait()
        .unwrap_or_else(|error| panic!("waiting for {binary} failed: {error}"));
    // The `pretty` format writes ANSI colour sequences between a field name and its value, so the
    // text has to be stripped before any of it can be matched.
    let out = strip_ansi(&drain(child.stdout.take()));
    let log = format!(
        "--- stdout ---\n{out}--- stderr ---\n{}",
        drain(child.stderr.take())
    );

    assert!(
        ready,
        "{binary} never answered 200 on http://127.0.0.1:{admin_port}/health/ready\n{log}"
    );
    assert_eq!(
        status.code(),
        Some(0),
        "{binary} did not exit 0 after SIGTERM ({status})\n{log}"
    );
    // The two spans the lifecycle requires, on the stream they are actually written to.
    assert!(
        out.contains("startup complete"),
        "{binary} logged no startup line\n{log}"
    );
    assert!(
        out.contains("\"graceful\":true") || out.contains("graceful: true"),
        "{binary} logged no graceful shutdown\n{log}"
    );
    out
}

/// `text` without ANSI control sequences.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            // A CSI sequence runs to its first alphabetic byte, which is `m` for every colour.
            for next in characters.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(character);
        }
    }
    out
}

/// The path of a workspace binary, resolved beside this package's own one.
///
/// `CARGO_BIN_EXE_*` is set only for the binaries of the package under test, so the other one is
/// found by name in the same directory. Only a build puts them there: `cargo build --workspace`
/// does, and `cargo test --workspace` does NOT — it builds the binary of the package whose tests it
/// is running, never a sibling package's plain binary. The gate therefore runs `cargo build
/// --workspace --locked` before `cargo test`, and DEVELOPMENT.md records why.
fn built_binary(binary: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_ratatoskr-telegram-webhook")).with_file_name(binary);
    assert!(
        path.is_file(),
        "{} has not been built; run `cargo build --workspace` first (`cargo test` does not build \
         a sibling package's binary)",
        path.display()
    );
    path
}

/// Polls `/health/ready` until it answers `200` with `"state":"ready"`, or the timeout expires.
///
/// A `503` early on is expected and not a failure: readiness is `not_ready` between the admin
/// listener binding and `mark_startup_complete`.
fn wait_until_ready(admin_port: u16) -> bool {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        let (status, body) = probe(admin_port, "/health/ready");
        if let Some(status) = status
            && status.starts_with("HTTP/1.1 200")
            && body.contains("\"state\":\"ready\"")
        {
            return true;
        }
        sleep(POLL_INTERVAL);
    }
    false
}

/// One `GET path` written onto a raw socket; `(status line, body)`.
///
/// The admin plane speaks plain HTTP/1.1 and `Connection: close` makes the whole response readable
/// to EOF, so a client library would be the only dependency this package has.
fn probe(admin_port: u16, path: &str) -> (Option<String>, String) {
    let Ok(mut socket) = TcpStream::connect(("127.0.0.1", admin_port)) else {
        return (None, String::new());
    };
    if socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .is_err()
    {
        return (None, String::new());
    }
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    if socket.write_all(request.as_bytes()).is_err() {
        return (None, String::new());
    }
    let mut response = String::new();
    if socket.read_to_string(&mut response).is_err() {
        return (None, String::new());
    }
    let status = response.lines().next().map(ToOwned::to_owned);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}

/// Polls `try_wait` until the child exits or the timeout expires; `None` means still running.
fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        sleep(POLL_INTERVAL);
    }
    child.try_wait().ok().flatten()
}

/// Sends SIGTERM, the signal the shutdown sequence listens for.
///
/// `Child::kill` sends SIGKILL, which skips the drain entirely and never yields exit `0`, and
/// `libc::kill` is unavailable because the workspace forbids unsafe code. `kill(1)` is the
/// remaining route and it is the same command `DEVELOPMENT.md` documents.
fn terminate(child: &mut Child) {
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill(1) is available on any unix host");
    assert!(status.success(), "SIGTERM could not be delivered: {status}");
}

/// Everything the child wrote to one stream. Read after `wait`, so the pipe is complete; a startup
/// and shutdown log is a few kilobytes and cannot fill it.
fn drain(stream: Option<impl Read>) -> String {
    let mut text = String::new();
    if let Some(mut stream) = stream {
        let _ = stream.read_to_string(&mut text);
    }
    text
}
