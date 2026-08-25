//! The dispatcher configuration section: parse, defaults, bounds, and the role's database
//! requirement.

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

/// The environment a dispatcher-role load needs besides the value under test: since item 4 the
/// role writes through the database, so its requirement must be satisfied for every other
/// assertion to be about the value alone.
fn admit_dispatcher_basics(jail: &mut Jail) {
    jail.set_env(
        "RATATOSKR__DATABASE__URL",
        "postgres://telegram@127.0.0.1:5432/telegram",
    );
}

/// The dispatcher section parses with every default filled, and an unknown key inside it is an
/// extraction error that names the key — the same refusal every other section gives.
#[test]
fn dispatcher_section_parses_with_defaults_and_unknown_keys_refused() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        admit_dispatcher_basics(jail);
        let config = config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .expect("dispatcher defaults must parse and validate");
        let dispatcher = &config.dispatcher;
        assert_eq!(dispatcher.global_messages_per_second, 25);
        assert_eq!(dispatcher.per_chat_min_interval_ms, 1200);
        assert_eq!(dispatcher.render_interval_secs, 4);
        assert_eq!(dispatcher.max_attempts, 5);
        assert_eq!(dispatcher.backoff_base_secs, 2);
        assert_eq!(dispatcher.backoff_cap_secs, 300);
        assert_eq!(dispatcher.jitter_fraction_milli, 200);
        assert_eq!(dispatcher.lease_ttl_secs, 60);
        assert_eq!(dispatcher.poll_idle_ms, 1000);

        jail.set_env("RATATOSKR__DISPATCHER__NOT_A_REAL_KEY", "1");
        let error = config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .expect_err("an unknown dispatcher key must be refused");
        let report = error.report(RuntimeRole::Dispatcher);
        assert!(report.contains("not_a_real_key"), "{report}");
        Ok(())
    });
}

/// Every limit field refuses zero, the inverted backoff bound refuses base above cap, and the
/// jitter fraction refuses above half; each violation names its field exactly.
#[test]
fn dispatcher_limits_refuse_zero_negative_and_inverted_bounds() {
    Jail::expect_with(|jail| {
        for (env_suffix, key) in [
            (
                "GLOBAL_MESSAGES_PER_SECOND",
                "dispatcher.global_messages_per_second",
            ),
            (
                "PER_CHAT_MIN_INTERVAL_MS",
                "dispatcher.per_chat_min_interval_ms",
            ),
            ("RENDER_INTERVAL_SECS", "dispatcher.render_interval_secs"),
            ("MAX_ATTEMPTS", "dispatcher.max_attempts"),
            ("BACKOFF_BASE_SECS", "dispatcher.backoff_base_secs"),
            ("BACKOFF_CAP_SECS", "dispatcher.backoff_cap_secs"),
            ("LEASE_TTL_SECS", "dispatcher.lease_ttl_secs"),
            ("POLL_IDLE_MS", "dispatcher.poll_idle_ms"),
        ] {
            jail.clear_env();
            admit_dispatcher_basics(jail);
            jail.set_env(format!("RATATOSKR__DISPATCHER__{env_suffix}"), "0");
            let error = config::load_from(
                RuntimeRole::Dispatcher,
                config::figment(RuntimeRole::Dispatcher),
            )
            .expect_err("zero must be refused");
            let config::ConfigError::Invalid(violations) = &error else {
                panic!("expected semantic violations for {key}, got {error:?}");
            };
            let violation = violations
                .iter()
                .find(|violation| violation.key == key)
                .unwrap_or_else(|| panic!("{key} must be named in {violations:?}"));
            assert_eq!(
                violation.env_var,
                format!("RATATOSKR__DISPATCHER__{env_suffix}"),
                "{key}"
            );
        }

        // The inverted bound: a cap below the base can never honour the base.
        jail.clear_env();
        admit_dispatcher_basics(jail);
        jail.set_env("RATATOSKR__DISPATCHER__BACKOFF_BASE_SECS", "10");
        jail.set_env("RATATOSKR__DISPATCHER__BACKOFF_CAP_SECS", "5");
        let error = config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .expect_err("cap below base must be refused");
        assert!(
            error
                .report(RuntimeRole::Dispatcher)
                .contains("backoff_cap_secs"),
            "the inverted bound names the cap"
        );

        // The jitter fraction ceiling: half the delay is the most jitter ever useful.
        jail.clear_env();
        admit_dispatcher_basics(jail);
        jail.set_env("RATATOSKR__DISPATCHER__JITTER_FRACTION_MILLI", "501");
        let error = config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .expect_err("jitter above half must be refused");
        assert!(
            error
                .report(RuntimeRole::Dispatcher)
                .contains("jitter_fraction_milli"),
            "the fraction violation names the field"
        );

        // The two ceilings: a per-chat gap above a minute starves a chat; a render interval
        // above an hour is not progress reporting.
        for (env_suffix, key) in [
            ("PER_CHAT_MIN_INTERVAL_MS", "per_chat_min_interval_ms"),
            ("RENDER_INTERVAL_SECS", "render_interval_secs"),
        ] {
            jail.clear_env();
            admit_dispatcher_basics(jail);
            jail.set_env(
                format!("RATATOSKR__DISPATCHER__{env_suffix}"),
                u64::MAX.to_string(),
            );
            let error = config::load_from(
                RuntimeRole::Dispatcher,
                config::figment(RuntimeRole::Dispatcher),
            )
            .expect_err("above the ceiling must be refused");
            assert!(
                error.report(RuntimeRole::Dispatcher).contains(key),
                "{key} must be named"
            );
        }
        Ok(())
    });
}

/// Since item 4 the dispatcher writes through `PostgreSQL`: without a database URL it refuses to
/// start, naming `DATABASE__URL`; with one, it validates cleanly.
#[test]
fn the_dispatcher_role_requires_a_database_url() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        let error = config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .expect_err("a dispatcher without a database must be refused");
        let config::ConfigError::Invalid(violations) = &error else {
            panic!("expected semantic violations, got {error:?}");
        };
        let violation = violations
            .iter()
            .find(|violation| violation.key == "database.url")
            .expect("the missing database url to be named");
        assert_eq!(violation.env_var, "RATATOSKR__DATABASE__URL");

        admit_dispatcher_basics(jail);
        config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .expect("a dispatcher with a database url validates");
        Ok(())
    });
}

/// The webhook role ignores the dispatcher section entirely: its own requirements are unchanged
/// whether or not dispatcher tuning is present.
#[test]
fn the_webhook_role_validates_without_any_dispatcher_section() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("RATATOSKR__BOT_API__TOKEN", "123456:TEST-dispatcher-token");
        jail.set_env(
            "RATATOSKR__WEBHOOK__SECRET_TOKEN",
            "webhook-secret-0123456789abcdef",
        );
        jail.set_env(
            "RATATOSKR__DATABASE__URL",
            "postgres://telegram@127.0.0.1:5432/telegram",
        );
        jail.set_env("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "700100200");
        let config = config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
            .expect("a webhook without dispatcher tuning validates");
        assert_eq!(config.dispatcher.global_messages_per_second, 25);
        Ok(())
    });
}
