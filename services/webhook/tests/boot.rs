//! Every deployable boots on the configuration `DEVELOPMENT.md` documents.
//!
//! This is the test that runs the shipped binaries as processes. It exists so that the local-run
//! block of `DEVELOPMENT.md` cannot rot: the admin plane is probed over a real socket, and the
//! documented SIGTERM shutdown is asserted to exit `0`.
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

/// How long a binary may take to answer `/health/ready` with `200`. Generous: a loaded CI runner
/// starting a cold process is the slow case, and the cost of a too-short timeout is a flake.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Between readiness polls.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Each role starts on its documented environment, reports ready on its documented admin port, and
/// exits `0` on SIGTERM after the drain. One test rather than two so the roles run sequentially:
/// they bind fixed ports, which is the point — those ports are the ones `DEVELOPMENT.md` tells an
/// operator to use.
#[test]
fn each_role_boots_on_its_documented_configuration_and_reports_ready() {
    boots(
        "ratatoskr-telegram-webhook",
        &[("RATATOSKR__TELEMETRY__LOG_FORMAT", "pretty")],
        9467,
    );

    boots("ratatoskr-telegram-dispatcher", &[], 9468);
}

/// A role with a configured but unreachable database reports the database check failing and stays
/// unready, rather than starting into a dependency it cannot see.
#[test]
fn an_unreachable_database_keeps_readiness_failing_and_names_the_check() {
    const PORT: u16 = 9477;
    let path = built_binary("ratatoskr-telegram-webhook");
    let mut child = Command::new(&path)
        .env("RATATOSKR__ADMIN__BIND", format!("127.0.0.1:{PORT}"))
        // Port 5 on loopback: nothing listens there, so connect fails fast instead of timing out.
        .env(
            "RATATOSKR__DATABASE__URL",
            "postgres://nobody:nope@127.0.0.1:5/nowhere",
        )
        .env("RATATOSKR__TELEMETRY__LOG_FORMAT", "pretty")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary must spawn");

    // The process must still be alive and serving liveness: an unreachable dependency is a
    // readiness fact, not a crash.
    let live = poll_status(PORT, "/health/live", 200, READY_TIMEOUT);
    let (_, ready_body) = probe(PORT, "/health/ready");
    terminate(&mut child);
    let status = child.wait().expect("the child must be waitable");
    let log = format!(
        "--- stdout ---\n{}--- stderr ---\n{}",
        drain(child.stdout.take()),
        drain(child.stderr.take()),
    );

    assert!(live, "liveness never answered 200\n{log}");
    assert!(
        ready_body.contains("\"name\":\"database\"")
            && ready_body.contains("dependency_unavailable"),
        "the database check must be failing by name\n{ready_body}",
    );
    assert_eq!(status.code(), Some(0), "SIGTERM must still exit 0\n{log}");
}

/// `check-config` is the documented init-container and CI pre-flight, so its exit codes are an
/// operational contract: `0` valid, `78` invalid, and the report never quotes a supplied value.
#[test]
fn check_config_exits_zero_on_a_valid_configuration_and_78_on_an_invalid_one() {
    // Wired into each main separately, so each is exercised.
    for binary in [
        "ratatoskr-telegram-webhook",
        "ratatoskr-telegram-dispatcher",
    ] {
        let output = Command::new(built_binary(binary))
            .arg("check-config")
            .env_remove("RATATOSKR__DATABASE__URL")
            .output()
            .expect("check-config must run");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{binary} with defaults: {}",
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let invalid = Command::new(built_binary("ratatoskr-telegram-webhook"))
        .arg("check-config")
        .env(
            "RATATOSKR__DATABASE__URL",
            "mysql://user:secret@db.example:3306/x",
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

/// Spawns `binary` with `env`, waits for readiness on `admin_port`, sends SIGTERM, and asserts a
/// clean exit. Both streams are reported with every failure: stdout carries the log records, stderr
/// only what is written before a subscriber exists.
fn boots(binary: &str, env: &[(&str, &str)], admin_port: u16) {
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

/// Polls one path until it answers `status`, or the timeout expires.
fn poll_status(admin_port: u16, path: &str, status: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(response) = probe(admin_port, path).0
            && response.starts_with(&format!("HTTP/1.1 {status}"))
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
