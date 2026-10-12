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
fn systemd_units_preserve_required_https_egress() {
    let webhook = read("systemd/ratatoskr-telegram-webhook.service");
    let dispatcher = read("systemd/ratatoskr-telegram-dispatcher.service");
    let webhook_env = read("systemd/webhook.conf.example");
    let dispatcher_env = read("systemd/dispatcher.conf.example");

    assert!(
        !webhook.contains("IPAddressDeny=any"),
        "webhook unit denies required public HTTPS egress"
    );
    assert!(
        !dispatcher.contains("IPAddressDeny=any"),
        "dispatcher unit denies required public HTTPS egress"
    );
    for (role, unit) in [("webhook", &webhook), ("dispatcher", &dispatcher)] {
        assert!(
            unit.contains("RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK"),
            "{role} unit lost its address-family boundary"
        );
    }
    assert!(webhook_env.contains("RATATOSKR__WEBHOOK__BIND=127.0.0.1:8182"));
    assert!(webhook_env.contains("RATATOSKR__ADMIN__BIND=0.0.0.0:9467"));
    assert!(dispatcher_env.contains("RATATOSKR__ADMIN__BIND=0.0.0.0:9468"));
    assert!(!dispatcher_env.contains("RATATOSKR__WEBHOOK__BIND"));
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

/// The shipped examples name the audience Edge accepts and the port the workspace allocates to
/// Edge (XR-021 CONTRACTS S11); the old audience and the unallocated default are gone.
#[test]
fn shipped_examples_target_the_allocated_edge() {
    for (role, example) in [
        ("webhook", read("systemd/webhook.conf.example")),
        ("dispatcher", read("systemd/dispatcher.conf.example")),
    ] {
        let lines: Vec<&str> = example.lines().collect();
        for expected in [
            "RATATOSKR__PLATFORM__AUDIENCE=edge",
            "RATATOSKR__PLATFORM__BASE_URL=http://127.0.0.1:8080",
        ] {
            assert!(
                lines.contains(&expected),
                "{role} example lacks exactly {expected}\n{example}"
            );
        }
        assert!(
            !example.contains("ratatoskr-platform"),
            "{role} example still names the old audience\n{example}"
        );
    }
}

/// `.env.example` documents the compiled listener default, not another service's operator port
/// (XR-021 CONTRACTS S05: 9469 belongs to channel-digests).
#[test]
fn env_example_documents_the_compiled_listener_defaults() {
    let example = std::fs::read_to_string(root().join(".env.example")).expect(".env.example");
    assert!(
        example.contains("RATATOSKR__WEBHOOK__BIND=127.0.0.1:8182"),
        "the webhook bind default is not documented as 127.0.0.1:8182"
    );
    assert!(
        example.contains("Default 127.0.0.1:8182"),
        "the webhook bind comment does not state the default 127.0.0.1:8182"
    );
    assert!(
        !example.contains("9469"),
        "9469 is channel-digests' API operator port and must not appear"
    );
}

/// The attachment root lives on the durable device and is group-readable so the extractor can read
/// Telegram-owned bytes by `BlobRef` without a transfer service (XR-021 CONTRACTS S11). The unit
/// keeps its existing `/var/lib` write path; the new root is added beside it.
#[test]
fn attachment_root_is_on_the_durable_device_and_group_readable() {
    const ROOT: &str = "/mnt/nvme/ratatoskr/blobs/ratatoskr-telegram";
    let webhook_env = read("systemd/webhook.conf.example");
    let webhook = read("systemd/ratatoskr-telegram-webhook.service");
    let guide = read("README.md");

    assert!(
        webhook_env
            .lines()
            .any(|line| line == format!("RATATOSKR__INGESTION__BLOB_ROOT={ROOT}")),
        "the attachment root is not on the durable device\n{webhook_env}"
    );
    let read_write = webhook
        .lines()
        .find(|line| line.starts_with("ReadWritePaths="))
        .expect("the unit declares ReadWritePaths");
    assert!(
        read_write.starts_with("ReadWritePaths=/var/lib/ratatoskr-telegram")
            && read_write.split_whitespace().any(|path| path == ROOT),
        "ReadWritePaths does not list the attachment root: {read_write}"
    );
    assert!(webhook.contains("\nUMask=0027\n"), "UMask is not 0027");
    let groups = webhook
        .lines()
        .find(|line| line.starts_with("SupplementaryGroups="))
        .expect("the unit declares SupplementaryGroups");
    assert!(
        groups
            .split_whitespace()
            .any(|group| group.trim_start_matches("SupplementaryGroups=")
                == "ratatoskr-telegram-blobs"),
        "SupplementaryGroups lacks ratatoskr-telegram-blobs: {groups}"
    );
    assert!(
        guide.contains("-o ratatoskr-telegram-webhook -g ratatoskr-telegram-blobs -m 2750")
            && guide.contains(ROOT),
        "the guide does not document the setgid 2750 install step"
    );
}

/// The dispatcher's NATS seed is installed at `/etc/ratatoskr/<identity>.nkey`, where Platform's
/// NATS README and every other service of the fleet put theirs (XR-021 CONTRACTS R2-10). The bot
/// token and the assertion signing key keep their `/etc/ratatoskr/secrets/` paths.
#[test]
fn dispatcher_example_installs_the_seed_where_the_fleet_does() {
    const SEED: &str = "/etc/ratatoskr/telegram.nkey";
    let dispatcher_env = read("systemd/dispatcher.conf.example");
    let guide = read("README.md");

    assert!(
        dispatcher_env
            .lines()
            .any(|line| line == format!("RATATOSKR__NOTIFICATION_BUS__CREDENTIALS_FILE={SEED}")),
        "the dispatcher example does not point the seed at {SEED}\n{dispatcher_env}"
    );
    assert!(
        guide.contains("install -m 0640 -o root -g ratatoskr-telegram-dispatcher")
            && guide.contains(SEED),
        "the guide does not install the seed at {SEED} for the dispatcher group"
    );
    assert!(
        !format!("{dispatcher_env}\n{guide}").contains("secrets/telegram.nkey"),
        "a stale seed path under /etc/ratatoskr/secrets remains"
    );
    for kept in [
        "RATATOSKR__BOT_API__TOKEN_FILE=/etc/ratatoskr/secrets/telegram-bot-token",
        "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY_FILE=/etc/ratatoskr/secrets/telegram-platform-signing-key",
    ] {
        assert!(
            dispatcher_env.lines().any(|line| line == kept),
            "{kept} moved out of /etc/ratatoskr/secrets"
        );
    }
}

/// The assertion key has a documented procedure: generate once on the host with the webhook
/// binary, install the seed for the shared secrets group, and paste the printed public key into
/// the Edge configuration (XR-021 CONTRACTS R3-12). Each example points at the section beside the
/// key-file line.
#[test]
fn the_deploy_readme_documents_the_assertion_key_procedure() {
    const KEY_FILE: &str = "/etc/ratatoskr/secrets/telegram-platform-signing-key";
    let guide = read("README.md");
    assert!(
        guide.contains("## Platform assertion key"),
        "the guide has no Platform assertion key section"
    );
    for needed in [
        "ratatoskr-telegram-webhook assertion-key generate",
        "ratatoskr-telegram-webhook assertion-key public",
        KEY_FILE,
        "root:ratatoskr-telegram-secrets 0640",
        "RATATOSKR__IDENTITY__ASSERTION_KEY",
        "/etc/ratatoskr/edge.conf",
    ] {
        assert!(guide.contains(needed), "the guide does not name {needed}");
    }
    for (role, example) in [
        ("webhook", read("systemd/webhook.conf.example")),
        ("dispatcher", read("systemd/dispatcher.conf.example")),
    ] {
        let lines: Vec<&str> = example.lines().collect();
        let index = lines
            .iter()
            .position(|line| {
                *line == format!("RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY_FILE={KEY_FILE}")
            })
            .unwrap_or_else(|| panic!("{role} example lost the key file line"));
        let pointer = index.checked_sub(1).and_then(|before| lines.get(before));
        assert!(
            pointer.is_some_and(
                |line| line.starts_with('#') && line.contains("Platform assertion key")
            ),
            "{role} example does not point at the Platform assertion key section above the key \
             file line\n{example}"
        );
    }
}

/// Both examples name the one role and database that `deploy/postgres/role.sql` provisions: the
/// webhook and the dispatcher share `ratatoskr_telegram` on `ratatoskr`, port 5432 (XR-021
/// CONTRACTS R3-11). The file creates a restricted login role and revokes its public-schema rights.
#[test]
fn the_role_file_creates_the_role_both_examples_name() {
    let role_file = read("postgres/role.sql");
    let guide = read("README.md");
    let mut named = Vec::new();
    for (name, example) in [
        ("webhook", read("systemd/webhook.conf.example")),
        ("dispatcher", read("systemd/dispatcher.conf.example")),
    ] {
        let url = example
            .lines()
            .find_map(|line| line.strip_prefix("RATATOSKR__DATABASE__URL="))
            .unwrap_or_else(|| panic!("{name} example has no database URL"));
        let rest = url
            .strip_prefix("postgres://")
            .unwrap_or_else(|| panic!("{name} database URL is not postgres://"));
        let (credentials, location) = rest.split_once('@').expect("credentials and location");
        let (role, _password) = credentials.split_once(':').expect("role and password");
        let (host_port, database) = location.split_once('/').expect("host and database");
        assert_eq!(
            host_port, "127.0.0.1:5432",
            "{name} must use the target host's port"
        );
        named.push((role.to_owned(), database.to_owned()));
    }
    assert_eq!(
        named[0], named[1],
        "the webhook and the dispatcher share one role"
    );
    let (role, database) = &named[0];
    assert_eq!(
        (role.as_str(), database.as_str()),
        ("ratatoskr_telegram", "ratatoskr")
    );

    for needed in [
        format!("create role {role} login"),
        format!("alter role {role} nosuperuser nocreatedb nocreaterole noreplication nobypassrls;"),
        format!("grant connect, create on database {database} to {role};"),
        format!("\\connect {database}"),
        format!("revoke all on schema public from {role};"),
    ] {
        assert!(role_file.contains(&needed), "role.sql lacks: {needed}");
    }
    assert!(
        !role_file.to_lowercase().contains("password '"),
        "role.sql must not carry a credential; the password is set with \\password"
    );
    assert!(
        guide.contains("deploy/postgres/role.sql")
            && guide.contains("\\password ratatoskr_telegram"),
        "the guide does not describe running role.sql and setting the password"
    );
    assert!(
        guide.contains("Edge first, then `deploy/postgres/role.sql`, then the Telegram services"),
        "the guide must state the provisioning order"
    );
}

/// Edge fixes the notification durable's ack wait at 30 seconds and the dispatcher refuses any
/// other value, so the example states the number and why, and the guide names the key
/// (XR-021 CONTRACTS R4-08 rule 4).
#[test]
fn the_dispatcher_example_states_why_the_ack_wait_is_thirty() {
    const SETTING: &str = "RATATOSKR__NOTIFICATION_BUS__ACK_WAIT_SECONDS=30";
    let example = read("systemd/dispatcher.conf.example");
    let guide = read("README.md");
    let lines: Vec<&str> = example.lines().collect();
    let index = lines
        .iter()
        .position(|line| *line == SETTING)
        .unwrap_or_else(|| panic!("the dispatcher example does not set {SETTING}\n{example}"));
    let comment = index
        .checked_sub(1)
        .and_then(|before| lines.get(before))
        .copied()
        .unwrap_or_default();
    assert!(
        comment.starts_with('#')
            && comment.contains("ratatoskr_telegram_notifications")
            && comment.contains("30 seconds")
            && comment.contains("any other value stops the dispatcher"),
        "the line above the ack wait does not explain it: {comment:?}"
    );
    for needed in [
        "RATATOSKR__NOTIFICATION_BUS__ACK_WAIT_SECONDS",
        "ratatoskr_telegram_notifications",
        "30 seconds",
    ] {
        assert!(
            guide.contains(needed),
            "the guide does not mention {needed}"
        );
    }
}
