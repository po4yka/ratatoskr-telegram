//! The Platform configuration section: parsing with defaults, value rules, and the both-roles
//! requirement. Every refusal names keys and never echoes supplied values.

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
use secrecy::ExposeSecret as _;
use telegram_core::RuntimeRole;
use telegram_core::config;

const VALID_KEY: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
const AUDIENCE_TEST: &str = "ratatoskr-edge-test";

/// The section parses with its documented defaults; validation then demands the secrets, which
/// proves parsing reached it and the defaults are the development-harness values.
#[test]
fn platform_section_parses_with_defaults_and_unknown_keys_refused() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env(
            "RATATOSKR__DATABASE__URL",
            "postgres://telegram@127.0.0.1:5432/telegram",
        );
        let error = config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .expect_err("the empty secrets must be refused");
        let report = error.report(RuntimeRole::Dispatcher);
        assert!(report.contains("platform.audience"), "{report}");
        assert!(
            report.contains("platform.assertion_signing_key"),
            "{report}"
        );
        Ok(())
    });
}

/// An unknown platform key refuses the load, named in the report.
#[test]
fn an_unknown_platform_key_is_refused_and_named() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RATATOSKR__PLATFORM__NO_SUCH_KEY", "x");
        let Some(error) =
            config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook)).err()
        else {
            panic!("an unknown key must not parse");
        };
        assert!(
            error
                .report(RuntimeRole::Webhook)
                .contains("PLATFORM__NO_SUCH_KEY"),
            "the report names the key"
        );
        Ok(())
    });
}

/// Value rules name keys without echoing values, for every field of the section.
#[test]
fn platform_value_rules_violations_name_keys_without_echoing_secrets() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env(
            "RATATOSKR__PLATFORM__BASE_URL",
            "http://platform.example.test",
        );
        jail.set_env(
            "RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY",
            "not-hex-at-all",
        );
        let long_audience = "a".repeat(200);
        jail.set_env("RATATOSKR__PLATFORM__AUDIENCE", long_audience.as_str());
        jail.set_env("RATATOSKR__PLATFORM__TIMEOUT_SECONDS", "0");

        let Some(error) =
            config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook)).err()
        else {
            panic!("every rule above must refuse");
        };
        let report = error.report(RuntimeRole::Webhook);
        for key in [
            "platform.base_url",
            "platform.timeout_seconds",
            "platform.audience",
            "platform.assertion_signing_key",
        ] {
            assert!(report.contains(key), "{key} not named:\n{report}");
        }
        assert!(
            !report.contains("not-hex-at-all"),
            "the signing key leaked into the report:\n{report}"
        );
        Ok(())
    });
}

/// Both runtime roles demand the Platform section's audience and signing key.
#[test]
fn both_roles_require_the_platform_section() {
    for role in [RuntimeRole::Webhook, RuntimeRole::Dispatcher] {
        Jail::expect_with(|jail| {
            jail.clear_env();
            if role == RuntimeRole::Webhook {
                jail.set_env("RATATOSKR__BOT_API__TOKEN", "123456:TEST-token");
                jail.set_env(
                    "RATATOSKR__WEBHOOK__SECRET_TOKEN",
                    "webhook-secret-0123456789abcdef",
                );
                jail.set_env("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "700100200");
            }
            jail.set_env(
                "RATATOSKR__DATABASE__URL",
                "postgres://telegram@127.0.0.1:5432/telegram",
            );
            let error = config::load_from(role, config::figment(role))
                .err()
                .unwrap_or_else(|| panic!("an unconfigured platform section must refuse"));
            let report = error.report(role);
            assert!(
                report.contains("platform.audience")
                    && report.contains("platform.assertion_signing_key"),
                "{role} must name both platform secrets:\n{report}"
            );
            Ok(())
        });
    }
}

/// A fully configured section validates for either role with the secret intact in memory.
#[test]
fn a_configured_platform_section_validates() {
    for role in [RuntimeRole::Webhook, RuntimeRole::Dispatcher] {
        Jail::expect_with(|jail| {
            jail.clear_env();
            if role == RuntimeRole::Webhook {
                jail.set_env("RATATOSKR__BOT_API__TOKEN", "123456:TEST-token");
                jail.set_env(
                    "RATATOSKR__WEBHOOK__SECRET_TOKEN",
                    "webhook-secret-0123456789abcdef",
                );
                jail.set_env("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "700100200");
            }
            jail.set_env(
                "RATATOSKR__DATABASE__URL",
                "postgres://telegram@127.0.0.1:5432/telegram",
            );
            jail.set_env("RATATOSKR__PLATFORM__BASE_URL", "http://127.0.0.1:9463");
            jail.set_env("RATATOSKR__PLATFORM__AUDIENCE", AUDIENCE_TEST);
            jail.set_env("RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY", VALID_KEY);
            if role == RuntimeRole::Dispatcher {
                jail.set_env(
                    "RATATOSKR__NOTIFICATION_BUS__CREDENTIALS_FILE",
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("Cargo.toml")
                        .to_string_lossy()
                        .as_ref(),
                );
            }
            let config = config::load_from(role, config::figment(role))
                .unwrap_or_else(|error| panic!("{role} must validate: {}", error.report(role)));
            assert_eq!(config.platform.audience, AUDIENCE_TEST);
            assert_eq!(
                config.platform.assertion_signing_key.expose_secret(),
                VALID_KEY
            );
            Ok(())
        });
    }
}
