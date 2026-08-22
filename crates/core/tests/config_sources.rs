//! Where configuration comes from: defaults, environment overrides, unknown keys.

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

/// An empty environment loads per-role built-in defaults, including each role's own admin port.
#[test]
fn an_empty_environment_loads_the_role_defaults() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        for role in [RuntimeRole::Webhook, RuntimeRole::Dispatcher] {
            let config = config::load_from(role, config::figment(role)).expect("defaults load");
            assert_eq!(config.admin.bind.port(), role.default_admin_port());
            assert!(config.admin.bind.ip().is_loopback(), "deny by default");
            assert!(
                config.database.is_none(),
                "no database is configured by default",
            );
        }
        Ok(())
    });
}

/// One variable overrides exactly one field.
#[test]
fn one_variable_overrides_exactly_one_field() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RATATOSKR__ADMIN__BIND", "127.0.0.1:9998");
        let config = config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
            .expect("the override parses");
        assert_eq!(config.admin.bind.port(), 9998);
        // Everything else is untouched: the shutdown default is still the documented one.
        assert_eq!(config.shutdown.drain_seconds, 5);
        Ok(())
    });
}

/// Nested tables parse from `__`-joined variables.
#[test]
fn nested_tables_parse_from_joined_variables() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env(
            "RATATOSKR__DATABASE__URL",
            "postgres://telegram:telegram@127.0.0.1:5432/telegram",
        );
        jail.set_env("RATATOSKR__DATABASE__MAX_CONNECTIONS", "3");
        let config = config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
            .expect("the nested table parses");
        let database = config.database.expect("the database table is present");
        assert_eq!(database.max_connections, 3);
        Ok(())
    });
}

/// An unknown key is refused instead of ignored — the acceptance criterion this change exists for.
#[test]
fn an_unknown_field_is_refused_and_named() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RATATOSKR__NO_SUCH_FIELD", "x");
        let error = config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
            .expect_err("an unknown key must not parse");
        let report = error.report(RuntimeRole::Webhook);
        assert!(report.contains("NO_SUCH_FIELD"), "{report}");
        // The supplied value is never echoed back, so a typo cannot become a leak.
        assert!(!report.contains("\"x\""), "{report}");
        Ok(())
    });
}
