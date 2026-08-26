//! The ingestion limits: defaults, exact parsing, and named refusal bounds.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]
#![allow(
    clippy::result_large_err,
    reason = "a figment::Jail closure returns figment::Error; the size is figment's, not ours"
)]

use figment::Jail;
use telegram_core::config::{self, ConfigError};
use telegram_core::role::RuntimeRole;

/// The documented default budget: 18 MiB, under the Bot API's own 20 MiB download ceiling so
/// refusal happens in this service before Telegram's does.
const DEFAULT_BUDGET: u64 = 18 * 1_048_576;

/// The webhook role's minimal valid environment, minus the ingestion keys under test.
fn seed_role(jail: &mut Jail) {
    jail.clear_env();
    jail.set_env(
        "RATATOSKR__BOT_API__TOKEN",
        "123456:TEST-config-source-token",
    );
    jail.set_env(
        "RATATOSKR__WEBHOOK__SECRET_TOKEN",
        "webhook-secret-0123456789abcdef",
    );
    jail.set_env(
        "RATATOSKR__DATABASE__URL",
        "postgres://telegram@127.0.0.1:5432/telegram",
    );
    jail.set_env("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "700100200");
    jail.set_env("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463");
    jail.set_env("RATATOSKR__PLATFORM__AUDIENCE", "ratatoskr-edge-test");
    jail.set_env(
        "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
        "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
    );
}

/// Absent means the documented default; an in-range value loads exactly.
#[test]
fn the_ingestion_budget_defaults_and_parses_exactly() {
    let defaults = telegram_core::config::TelegramConfig::defaults(RuntimeRole::Webhook);
    assert_eq!(
        defaults.ingestion.max_attachment_bytes, DEFAULT_BUDGET,
        "the default sits under the Bot API ceiling"
    );

    Jail::expect_with(|jail| {
        seed_role(jail);
        jail.set_env("RATATOSKR__INGESTION__MAX_ATTACHMENT_BYTES", "1048576");
        let config = config::load(RuntimeRole::Webhook).expect("the override parses");
        assert_eq!(config.ingestion.max_attachment_bytes, 1_048_576);
        Ok(())
    });
}

/// Zero and above the Bot API ceiling are refused by name, without quoting the value.
#[test]
fn an_out_of_range_budget_is_refused_with_a_named_rule() {
    // The zero case proves refusal; the multi-digit case also proves the supplied value never
    // reaches the report (a single "0" cannot be checked this way - every bound in the report
    // contains that digit).
    Jail::expect_with(|jail| {
        seed_role(jail);
        jail.set_env("RATATOSKR__INGESTION__MAX_ATTACHMENT_BYTES", "0");
        let error =
            config::load(RuntimeRole::Webhook).expect_err("a zero budget must refuse to start");
        let ConfigError::Invalid(violations) = error else {
            panic!("expected Invalid, got {error:?}");
        };
        let report = config::ConfigError::Invalid(violations).report(RuntimeRole::Webhook);
        assert!(
            report.contains("ingestion.max_attachment_bytes"),
            "{report}"
        );
        Ok(())
    });
    Jail::expect_with(|jail| {
        seed_role(jail);
        jail.set_env("RATATOSKR__INGESTION__MAX_ATTACHMENT_BYTES", "22020096");
        let error = config::load(RuntimeRole::Webhook)
            .expect_err("an above-ceiling budget must refuse to start");
        let ConfigError::Invalid(violations) = error else {
            panic!("expected Invalid, got {error:?}");
        };
        let report = config::ConfigError::Invalid(violations).report(RuntimeRole::Webhook);
        assert!(
            report.contains("RATATOSKR__INGESTION__MAX_ATTACHMENT_BYTES"),
            "{report}"
        );
        assert!(
            !report.contains("22020096"),
            "the report must not quote the supplied value"
        );
        Ok(())
    });
}

/// Unknown ingestion fields are refused like every other section's.
#[test]
fn unknown_ingestion_fields_are_refused() {
    Jail::expect_with(|jail| {
        seed_role(jail);
        jail.set_env("RATATOSKR__INGESTION__MAX_TOTAL_BYTES", "1048576");
        config::load(RuntimeRole::Webhook).expect_err("an unknown key must not parse");
        Ok(())
    });
}
