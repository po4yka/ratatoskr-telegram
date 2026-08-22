//! The startup rules: every violation reported, no supplied value echoed.

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
use telegram_core::config;
use telegram_core::role::RuntimeRole;

/// Two invalid values produce two violations in ONE report — an operator editing an environment
/// wants one round trip, not five.
#[test]
fn two_invalid_values_produce_two_violations_in_one_report() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RATATOSKR__SHUTDOWN__GRACE_SECONDS", "0");
        jail.set_env(
            "RATATOSKR__TELEMETRY__OTLP__ENDPOINT",
            "ftp://collector.example:4317",
        );
        let error = config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
            .expect_err("both rules must fail");
        let violations = match &error {
            config::ConfigError::Invalid(violations) => violations,
            other => panic!("expected semantic violations, got {other:?}"),
        };
        assert_eq!(violations.len(), 2, "{violations:#?}");
        let report = error.report(RuntimeRole::Webhook);
        assert!(report.contains("shutdown.grace_seconds"), "{report}");
        assert!(report.contains("telemetry.otlp.endpoint"), "{report}");
        Ok(())
    });
}

/// A violation report names keys and rules, never the value that broke the rule.
#[test]
fn a_violation_report_never_quotes_the_supplied_value() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env(
            "RATATOSKR__DATABASE__URL",
            "mysql://user:secret-password@db.example:3306/x",
        );
        let error = config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
            .expect_err("the scheme rule must fail");
        let report = error.report(RuntimeRole::Webhook);
        assert!(report.contains("database.url"), "{report}");
        assert!(
            !report.contains("secret-password") && !report.contains("mysql://"),
            "the report echoed the supplied value: {report}",
        );
        Ok(())
    });
}

/// The defaults are valid for both roles; nothing to fix before a process can start.
#[test]
fn the_defaults_are_valid_for_every_role() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        for role in [RuntimeRole::Webhook, RuntimeRole::Dispatcher] {
            config::load_from(role, config::figment(role))
                .unwrap_or_else(|error| panic!("{role} defaults must validate: {error}"));
        }
        Ok(())
    });
}
