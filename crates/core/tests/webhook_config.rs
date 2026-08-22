//! The intake configuration rules: V9–V13, the role requirements, and the value-free report.

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

/// The environment of the webhook role once everything it requires is supplied, over clean
/// defaults.
fn webhook_env(jail: &mut Jail) {
    jail.clear_env();
    jail.set_env(
        "RATATOSKR__BOT_API__TOKEN",
        "123456:AA-test-token-value-0123456789",
    );
    jail.set_env(
        "RATATOSKR__WEBHOOK__SECRET_TOKEN",
        "webhook-secret-0123456789abcdef",
    );
    jail.set_env(
        "RATATOSKR__DATABASE__URL",
        "postgres://telegram@127.0.0.1:5432/telegram",
    );
}

fn violations_of(role: RuntimeRole) -> Vec<String> {
    let error = config::load_from(role, config::figment(role))
        .err()
        .unwrap_or_else(|| panic!("{role} was expected to refuse this configuration"));
    match error {
        ConfigError::Invalid(violations) => violations
            .into_iter()
            .map(|violation| violation.key.to_owned())
            .collect(),
        other => panic!("expected semantic violations, got {other:?}"),
    }
}

/// V9 — the Bot API endpoint is https unless its host is loopback, which is what a local harness
/// server is.
#[test]
fn the_bot_api_endpoint_is_https_unless_loopback() {
    Jail::expect_with(|jail| {
        webhook_env(jail);
        jail.set_env("RATATOSKR__BOT_API__BASE_URL", "http://api.telegram.org");

        let keys = violations_of(RuntimeRole::Webhook);
        assert!(keys.contains(&"bot_api.base_url".to_owned()), "{keys:#?}");

        jail.set_env("RATATOSKR__BOT_API__BASE_URL", "http://127.0.0.1:8080");
        config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
            .expect("a loopback http endpoint must validate for the harness");
        Ok(())
    });
}

/// V10 — the call timeout is bounded; zero and hour-long budgets are both misreadings.
#[test]
fn the_bot_api_timeout_is_bounded() {
    Jail::expect_with(|jail| {
        webhook_env(jail);
        for seconds in ["0", "61"] {
            jail.set_env("RATATOSKR__BOT_API__TIMEOUT_SECONDS", seconds);
            let keys = violations_of(RuntimeRole::Webhook);
            assert!(
                keys.contains(&"bot_api.timeout_seconds".to_owned()),
                "{seconds}: {keys:#?}"
            );
        }
        Ok(())
    });
}

/// V11 — the webhook secret is long enough to force entropy and stays inside Telegram's charset;
/// the report names the key without ever echoing the value.
#[test]
fn the_webhook_secret_has_a_floor_and_telegrams_charset() {
    Jail::expect_with(|jail| {
        webhook_env(jail);
        let marker = "short$ecret";
        jail.set_env("RATATOSKR__WEBHOOK__SECRET_TOKEN", marker);

        let keys = violations_of(RuntimeRole::Webhook);
        assert!(
            keys.contains(&"webhook.secret_token".to_owned()),
            "{keys:#?}"
        );
        let report = config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
            .expect_err("the secret rule must fail")
            .report(RuntimeRole::Webhook);
        assert!(report.contains("webhook.secret_token"), "{report}");
        assert!(
            !report.contains(marker),
            "the report echoed the supplied secret: {report}",
        );
        Ok(())
    });
}

/// V12 — the body cap is bounded between "big enough for any update" and "not a second unbounded
/// read"; the floor value itself is legal.
#[test]
fn the_body_cap_is_bounded() {
    Jail::expect_with(|jail| {
        webhook_env(jail);
        for bytes in ["0", "1048577"] {
            jail.set_env("RATATOSKR__WEBHOOK__MAX_BODY_BYTES", bytes);
            let keys = violations_of(RuntimeRole::Webhook);
            assert!(
                keys.contains(&"webhook.max_body_bytes".to_owned()),
                "{bytes}: {keys:#?}"
            );
        }
        jail.set_env("RATATOSKR__WEBHOOK__MAX_BODY_BYTES", "1024");
        config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
            .expect("the floor value itself must validate");
        Ok(())
    });
}

/// V13 — both listeners on one address would silently hide one of them.
#[test]
fn identical_admin_and_public_binds_are_refused() {
    Jail::expect_with(|jail| {
        webhook_env(jail);
        jail.set_env("RATATOSKR__ADMIN__BIND", "127.0.0.1:9469");
        jail.set_env("RATATOSKR__WEBHOOK__BIND", "127.0.0.1:9469");

        let keys = violations_of(RuntimeRole::Webhook);
        assert!(keys.contains(&"webhook.bind".to_owned()), "{keys:#?}");
        Ok(())
    });
}

/// V13 — the webhook role demands its token, its secret and its database by name, in one report,
/// when none are configured.
#[test]
fn the_webhook_role_names_every_missing_requirement() {
    Jail::expect_with(|jail| {
        jail.clear_env();

        let keys = violations_of(RuntimeRole::Webhook);
        for required in ["bot_api.token", "webhook.secret_token", "database.url"] {
            assert!(
                keys.contains(&required.to_owned()),
                "{required} not named: {keys:#?}"
            );
        }
        Ok(())
    });
}

/// V13 — the dispatcher carries no such requirement at this milestone: it loads on defaults.
#[test]
fn the_dispatcher_still_starts_unconfigured() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        config::load_from(
            RuntimeRole::Dispatcher,
            config::figment(RuntimeRole::Dispatcher),
        )
        .expect("dispatcher defaults must validate");
        Ok(())
    });
}

/// A fully configured webhook role validates, and the secrets render redacted in every output the
/// configuration can produce.
#[test]
fn a_configured_webhook_role_validates_with_secrets_redacted() {
    Jail::expect_with(|jail| {
        webhook_env(jail);
        let config_v =
            config::load_from(RuntimeRole::Webhook, config::figment(RuntimeRole::Webhook))
                .expect("the full webhook environment must validate");
        let rendered = format!("{config_v:#?}");
        assert!(
            !rendered.contains("webhook-secret-0123456789abcdef"),
            "{rendered}"
        );
        assert!(!rendered.contains("AA-test-token-value"), "{rendered}");
        Ok(())
    });
}
