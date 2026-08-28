//! Structural single-host deployment contract; no host mutation or credential is involved.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_owned()
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join("deploy").join(path))
        .unwrap_or_else(|error| panic!("deploy/{path}: {error}"))
}

fn assert_common_unit(unit: &str) {
    for directive in [
        "Type=exec",
        "TimeoutStopSec=130s",
        "Restart=always",
        "CPUQuota=100%",
        "TasksMax=128",
        "NoNewPrivileges=yes",
        "ProtectSystem=strict",
        "ProtectHome=yes",
        "PrivateTmp=yes",
        "StandardOutput=append:/mnt/nvme/ratatoskr/logs/",
    ] {
        assert!(unit.contains(directive), "missing {directive}\n{unit}");
    }
}

fn profile_valid(webhook: &str, dispatcher: &str, webhook_env: &str, dispatcher_env: &str) -> bool {
    webhook.contains("Type=exec")
        && dispatcher.contains("Type=exec")
        && webhook.contains("TimeoutStopSec=130s")
        && dispatcher.contains("TimeoutStopSec=130s")
        && webhook.contains("MemoryMax=512M")
        && dispatcher.contains("MemoryMax=384M")
        && dispatcher.contains("After=ratatoskr-telegram-webhook.service")
        && dispatcher.contains("Requires=ratatoskr-telegram-webhook.service")
        && webhook_env.contains("RATATOSKR__WEBHOOK__BIND=127.0.0.1:8182")
        && webhook_env.contains("RATATOSKR__ADMIN__BIND=0.0.0.0:9467")
        && dispatcher_env.contains("RATATOSKR__ADMIN__BIND=0.0.0.0:9468")
        && !dispatcher_env.contains("RATATOSKR__WEBHOOK__BIND")
}

#[test]
fn systemd_profile_matches_runtime_and_workspace_contract() {
    let webhook = read("systemd/ratatoskr-telegram-webhook.service");
    let dispatcher = read("systemd/ratatoskr-telegram-dispatcher.service");
    let webhook_env = read("systemd/webhook.conf.example");
    let dispatcher_env = read("systemd/dispatcher.conf.example");
    let logrotate = read("logrotate/ratatoskr-telegram");

    assert_common_unit(&webhook);
    assert_common_unit(&dispatcher);
    assert!(webhook.contains("MemoryHigh=384M") && webhook.contains("MemoryMax=512M"));
    assert!(dispatcher.contains("MemoryHigh=256M") && dispatcher.contains("MemoryMax=384M"));
    assert!(
        webhook.contains("ExecStartPre=/usr/local/bin/ratatoskr-telegram-webhook check-schema")
    );
    assert!(dispatcher.contains("After=ratatoskr-telegram-webhook.service"));
    assert!(dispatcher.contains("Requires=ratatoskr-telegram-webhook.service"));
    assert!(webhook.contains("ReadWritePaths=/var/lib/ratatoskr-telegram"));
    assert!(dispatcher.contains("ReadWritePaths=/mnt/nvme/ratatoskr/logs"));
    assert!(!dispatcher.contains("RATATOSKR__WEBHOOK__BIND"));
    assert!(webhook_env.contains("RATATOSKR__WEBHOOK__BIND=127.0.0.1:8182"));
    assert!(webhook_env.contains("RATATOSKR__ADMIN__BIND=0.0.0.0:9467"));
    assert!(dispatcher_env.contains("RATATOSKR__ADMIN__BIND=0.0.0.0:9468"));
    assert!(!dispatcher_env.contains("RATATOSKR__WEBHOOK__BIND"));
    for secret_file in [
        "TOKEN_FILE",
        "SECRET_TOKEN_FILE",
        "ASSERTION_SIGNING_KEY_FILE",
        "CREDENTIALS_FILE",
    ] {
        assert!(
            format!("{webhook_env}\n{dispatcher_env}").contains(secret_file),
            "secret file source {secret_file} absent"
        );
    }
    assert!(logrotate.contains("/mnt/nvme/ratatoskr/logs/telegram-*.log"));
    assert!(profile_valid(
        &webhook,
        &dispatcher,
        &webhook_env,
        &dispatcher_env
    ));
}

#[test]
fn deployment_profile_refuses_role_or_schema_drift() {
    let webhook = read("systemd/ratatoskr-telegram-webhook.service");
    let dispatcher = read("systemd/ratatoskr-telegram-dispatcher.service");
    let webhook_env = read("systemd/webhook.conf.example");
    let dispatcher_env = read("systemd/dispatcher.conf.example");
    let mutations = [
        (
            webhook.replace("TimeoutStopSec=130s", "TimeoutStopSec=90s"),
            dispatcher.clone(),
            webhook_env.clone(),
            dispatcher_env.clone(),
        ),
        (
            webhook.replace("MemoryMax=512M", ""),
            dispatcher.clone(),
            webhook_env.clone(),
            dispatcher_env.clone(),
        ),
        (
            webhook.clone(),
            dispatcher.replace("After=ratatoskr-telegram-webhook.service", ""),
            webhook_env.clone(),
            dispatcher_env.clone(),
        ),
        (
            webhook.clone(),
            dispatcher.clone(),
            webhook_env.replace("127.0.0.1:8182", "127.0.0.1:9469"),
            dispatcher_env.clone(),
        ),
        (
            webhook,
            dispatcher,
            webhook_env,
            dispatcher_env.replace("0.0.0.0:9468", "0.0.0.0:8182"),
        ),
    ];
    for (webhook, dispatcher, webhook_env, dispatcher_env) in mutations {
        assert!(
            !profile_valid(&webhook, &dispatcher, &webhook_env, &dispatcher_env),
            "mutated fixture unexpectedly satisfies the deployment contract"
        );
    }
}
