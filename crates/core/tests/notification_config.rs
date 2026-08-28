//! The finite notification-bus and file-backed secret configuration contract.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]
#![allow(
    clippy::result_large_err,
    reason = "a figment::Jail closure returns figment::Error"
)]

use figment::Jail;
use telegram_core::RuntimeRole;
use telegram_core::config;

const KEY: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";

fn credential_fixture(name: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("ratatoskr-telegram-{name}-{}", std::process::id()));
    std::fs::write(&path, "synthetic-nats-credential\n").expect("fixture write");
    path
}

fn seed_dispatcher(jail: &mut Jail, credentials: &std::path::Path) {
    jail.clear_env();
    jail.set_env(
        "RATATOSKR__DATABASE__URL",
        "postgres://telegram@127.0.0.1:5432/telegram",
    );
    jail.set_env("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test");
    jail.set_env("RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY", KEY);
    jail.set_env(
        "RATATOSKR__NOTIFICATION_BUS__ENDPOINT",
        "nats://127.0.0.1:4222",
    );
    jail.set_env("RATATOSKR__NOTIFICATION_BUS__STREAM", "ratatoskr_events");
    jail.set_env(
        "RATATOSKR__NOTIFICATION_BUS__DURABLE",
        "ratatoskr_telegram_notifications",
    );
    jail.set_env(
        "RATATOSKR__NOTIFICATION_BUS__SUBJECT",
        "evt.platform.notification.raised.v1",
    );
    jail.set_env("RATATOSKR__NOTIFICATION_BUS__FETCH_BATCH", "32");
    jail.set_env("RATATOSKR__NOTIFICATION_BUS__ACK_WAIT_SECONDS", "30");
    jail.set_env(
        "RATATOSKR__NOTIFICATION_BUS__CREDENTIALS_FILE",
        credentials.to_string_lossy().as_ref(),
    );
}

#[test]
fn canonical_notification_bus_configuration_is_finite_and_redacted() {
    let credentials = credential_fixture("canonical-nats-creds");
    Jail::expect_with(|jail| {
        seed_dispatcher(jail, &credentials);
        let loaded = config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .unwrap_or_else(|error| {
            panic!(
                "canonical configuration: {}",
                error.report(RuntimeRole::Dispatcher)
            )
        });
        assert!(!format!("{loaded:#?}").contains("synthetic-nats-credential"));
        Ok(())
    });
}

#[test]
fn wildcard_zero_remote_plaintext_and_conflicting_secret_sources_are_refused() {
    let credentials = credential_fixture("invalid-nats-creds");
    for (key, value, expected) in [
        (
            "RATATOSKR__NOTIFICATION_BUS__SUBJECT",
            "evt.>",
            "notification_bus.subject",
        ),
        (
            "RATATOSKR__NOTIFICATION_BUS__FETCH_BATCH",
            "0",
            "notification_bus.fetch_batch",
        ),
        (
            "RATATOSKR__NOTIFICATION_BUS__ENDPOINT",
            "nats://broker.example.test:4222",
            "notification_bus.endpoint",
        ),
    ] {
        Jail::expect_with(|jail| {
            seed_dispatcher(jail, &credentials);
            jail.set_env(key, value);
            let report = config::load_from(
                RuntimeRole::Dispatcher,
                config::figment(RuntimeRole::Dispatcher),
            )
            .expect_err("invalid bus configuration")
            .report(RuntimeRole::Dispatcher);
            assert!(report.contains(expected), "{report}");
            assert!(!report.contains(value), "supplied value leaked: {report}");
            Ok(())
        });
    }

    Jail::expect_with(|jail| {
        seed_dispatcher(jail, &credentials);
        jail.set_env("RATATOSKR__BOT_API__TOKEN", "123456:secret-value");
        let token_file = credential_fixture("bot-token");
        std::fs::write(&token_file, "123456:file-secret\n").expect("fixture write");
        jail.set_env(
            "RATATOSKR__BOT_API__TOKEN_FILE",
            token_file.to_string_lossy().as_ref(),
        );
        let report = config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .expect_err("two secret sources")
        .report(RuntimeRole::Dispatcher);
        assert!(report.contains("bot_api.token"), "{report}");
        assert!(!report.contains("secret-value") && !report.contains("file-secret"));
        Ok(())
    });
}
