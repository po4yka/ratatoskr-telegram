//! The `assertion-key` subcommand of the shipped webhook binary (XR-021 CONTRACTS.md section
//! R3-12): it generates the Platform assertion seed and prints the public key Edge needs, and it
//! never prints the seed.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output};

/// RFC 8032 section 7.1, test 1.
const VECTOR_SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const VECTOR_PUBLIC_BASE64: &str = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=";

fn webhook(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ratatoskr-telegram-webhook"))
        .args(arguments)
        .env_remove("RATATOSKR__DATABASE__URL")
        .output()
        .expect("the webhook binary runs")
}

fn path_argument(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

fn streams(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn public_prints_the_rfc_8032_vector() {
    let directory = tempfile::tempdir().expect("directory");
    let seed = directory.path().join("seed");
    std::fs::write(&seed, format!("{VECTOR_SEED}\n")).expect("seed file");

    let output = webhook(&["assertion-key", "public", path_argument(&seed)]);

    assert_eq!(output.status.code(), Some(0), "{}", streams(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{VECTOR_PUBLIC_BASE64}\n")
    );
    assert!(
        !streams(&output).contains(VECTOR_SEED),
        "the seed must never be printed"
    );
}

#[test]
fn generate_writes_a_private_seed_and_prints_its_public_key() {
    let directory = tempfile::tempdir().expect("directory");
    let seed = directory.path().join("telegram-platform-signing-key");

    let generated = webhook(&["assertion-key", "generate", path_argument(&seed)]);
    assert_eq!(generated.status.code(), Some(0), "{}", streams(&generated));

    let content = std::fs::read_to_string(&seed).expect("the seed file");
    assert_eq!(content.len(), 65, "64 characters and one newline");
    assert!(content.ends_with('\n'));
    assert!(
        content
            .trim_end()
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "lowercase hex"
    );
    assert_eq!(
        std::fs::metadata(&seed)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        !streams(&generated).contains(content.trim_end()),
        "generate must not print the seed"
    );

    let derived = webhook(&["assertion-key", "public", path_argument(&seed)]);
    assert_eq!(derived.status.code(), Some(0), "{}", streams(&derived));
    assert_eq!(
        generated.stdout, derived.stdout,
        "the printed key is the file's key"
    );
    assert_eq!(
        generated.stdout.len(),
        45,
        "44 base64 characters and a newline"
    );

    let again = webhook(&["assertion-key", "generate", path_argument(&seed)]);
    assert_ne!(
        again.status.code(),
        Some(0),
        "an existing seed is never replaced"
    );
    assert_eq!(std::fs::read_to_string(&seed).expect("unchanged"), content);
    assert!(!streams(&again).contains(content.trim_end()));
}

#[test]
fn public_refuses_a_malformed_seed_without_echoing_it() {
    let directory = tempfile::tempdir().expect("directory");
    let cases = [
        ("short", "9d61b19deffd5a60\n".to_owned()),
        ("non_hex", format!("{}\n", "z9".repeat(32))),
        ("long", format!("{VECTOR_SEED}00\n")),
        ("empty", String::new()),
    ];
    for (name, content) in cases {
        let seed = directory.path().join(name);
        std::fs::write(&seed, &content).expect("seed file");
        let output = webhook(&["assertion-key", "public", path_argument(&seed)]);
        assert_ne!(output.status.code(), Some(0), "{name} must be refused");
        assert!(output.stdout.is_empty(), "{name}: nothing on stdout");
        let seen = streams(&output);
        let text = content.trim();
        assert!(
            text.is_empty() || !seen.contains(text),
            "{name}: the seed text was echoed: {seen}"
        );
        assert!(
            seen.contains("assertion-key: "),
            "{name}: the refusal names the command: {seen}"
        );
    }
    let missing = webhook(&[
        "assertion-key",
        "public",
        path_argument(&directory.path().join("absent")),
    ]);
    assert_ne!(missing.status.code(), Some(0));
    assert!(missing.stdout.is_empty());
    assert!(streams(&missing).contains("assertion-key: "));
}

#[test]
fn an_unknown_subcommand_is_refused() {
    for arguments in [
        vec!["frobnicate"],
        vec!["assertion-key"],
        vec!["assertion-key", "rotate", "/tmp/never-created"],
        vec!["assertion-key", "public"],
        vec!["assertion-key", "generate"],
    ] {
        let output = webhook(&arguments);
        assert_eq!(
            output.status.code(),
            Some(64),
            "{arguments:?} is a usage error\n{}",
            streams(&output)
        );
        assert!(
            output.stdout.is_empty(),
            "{arguments:?}: usage goes to stderr"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("usage"),
            "{arguments:?}: stderr names the usage"
        );
    }
}

#[test]
fn check_config_is_still_dispatched() {
    let output = webhook(&["check-config"]);
    assert_eq!(
        output.status.code(),
        Some(78),
        "an unconfigured check-config is EX_CONFIG, not a usage error\n{}",
        streams(&output)
    );
}
