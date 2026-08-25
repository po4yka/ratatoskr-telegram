//! Typed configuration: the tree, the loader, and the startup rules.
//!
//! # Sources and precedence
//!
//! Two providers, lowest precedence first:
//!
//! 1. built-in defaults for the [`RuntimeRole`] — each role's admin port differs, which is what
//!    makes both binaries run in an empty environment;
//! 2. `RATATOSKR__` environment variables, with `__` separating nesting levels, e.g.
//!    `RATATOSKR__TELEMETRY__OTLP__ENDPOINT`.
//!
//! # There is deliberately no configuration file
//!
//! One mechanism and one place to look. No search path, no provenance check, no rule that a secret
//! may not come from a file — none of which can be wrong if there is no file. The deployment model
//! is a container reading its configuration from the environment. A lower-precedence file provider
//! is a backward-compatible one-line addition at the milestone an operator asks for one.
//!
//! What is not deferrable is the naming scheme: environment variable names are an operational
//! contract, and renaming them later breaks every deployment manifest in the fleet.

mod model;
mod validate;

use figment::Figment;
use figment::providers::{Env, Serialized};

pub use crate::config::model::{
    AccessConfig, AdminConfig, BotApiConfig, DatabaseConfig, DispatcherConfig, LogFormat,
    OtlpConfig, ShutdownConfig, TelegramConfig, TelemetryConfig, WebhookConfig,
};
pub use crate::config::validate::{SHUTDOWN_CEILING_SECONDS, Violation};
use crate::role::RuntimeRole;

/// The environment prefix, and the nesting separator inside it.
const ENV_PREFIX: &str = "RATATOSKR__";

/// Reads the process environment and produces a validated configuration for `role`.
///
/// Sources, lowest precedence first: built-in defaults for `role`, then `RATATOSKR__` environment
/// variables with `__` separating nesting levels. There is no configuration file (see the module
/// documentation for why).
///
/// # Errors
///
/// [`ConfigError::Source`] when extraction fails — a wrong type or an unknown key. figment is
/// fail-fast, so this reports exactly one problem, and it names both the key and the provider.
/// [`ConfigError::Invalid`] carrying EVERY semantic violation found, never only the first, because
/// an operator editing an environment wants one round trip and not five.
#[allow(
    clippy::result_large_err,
    reason = "figment::Error is the specified payload of ConfigError::Source; boxing it would hide \
              the key and provider it names behind an extra indirection for a value that is \
              constructed once, at startup, on the path that then exits"
)]
pub fn load(role: RuntimeRole) -> Result<TelegramConfig, ConfigError> {
    load_from(role, figment(role))
}

/// The provider stack [`load`] uses, exposed so a test can add a provider on top of it.
#[must_use]
pub fn figment(role: RuntimeRole) -> Figment {
    Figment::from(Serialized::defaults(TelegramConfig::defaults(role)))
        .merge(Env::prefixed(ENV_PREFIX).split("__"))
}

/// Extracts and validates from an arbitrary figment. The seam every configuration test uses.
///
/// # Errors
///
/// As [`load`].
#[allow(
    clippy::result_large_err,
    reason = "as `load`: the error payload is figment::Error by specification"
)]
#[allow(
    clippy::needless_pass_by_value,
    reason = "the figment is consumed so a caller cannot extract the same stack twice and reason \
              about two configurations in one process"
)]
pub fn load_from(role: RuntimeRole, figment: Figment) -> Result<TelegramConfig, ConfigError> {
    let config: TelegramConfig = figment.extract()?;
    let violations = validate::validate(role, &config);
    if violations.is_empty() {
        Ok(config)
    } else {
        Err(ConfigError::Invalid(violations))
    }
}

impl TelegramConfig {
    /// The built-in defaults for `role`. The ONLY place a default value is written.
    #[must_use]
    pub fn defaults(role: RuntimeRole) -> Self {
        let loopback = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
        Self {
            admin: AdminConfig {
                bind: std::net::SocketAddr::new(loopback, role.default_admin_port()),
            },
            // No owner by default: an owner id names a real Telegram principal, so there is no
            // default that is not either wrong or fabricated. The webhook role demands one (V14).
            access: AccessConfig::default(),
            // Absent by default, in every role. A database URL carries a credential, so there is
            // no default that is not either wrong or a secret in the source tree.
            database: None,
            bot_api: BotApiConfig::default(),
            dispatcher: model::DispatcherConfig::default(),
            // The intake listener is webhook-role configuration; its requirements are enforced by
            // rule V13 per role, not by a default that would silently satisfy them.
            webhook: None,
            // Windows that are safe on the smallest deployment: long enough that nothing is lost
            // to a weekend outage, short enough that no drain outlives its supervisor.
            shutdown: ShutdownConfig {
                drain_seconds: model::default_drain_seconds(),
                grace_seconds: model::default_grace_seconds(),
            },
            telemetry: TelemetryConfig {
                log_format: LogFormat::default(),
                log_filter: model::default_log_filter(),
                otlp: None,
            },
        }
    }
}

/// Every reason a Telegram process must refuse to start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Extraction failed: a wrong type, a missing key, or an unknown key. figment names both the
    /// key and the provider it came from, and is fail-fast, so this carries exactly one problem.
    ///
    /// figment's own message is deliberately NOT interpolated. It quotes the supplied value for
    /// several field types, so a `Display` that carried it would make one `tracing::error!(%error)`
    /// — on a tree that holds a `DATABASE_URL` — a live secret leak. [`ConfigError::report`] is the
    /// only operator-facing rendering, and it is value-free by construction.
    #[error("configuration could not be read")]
    Source(#[from] figment::Error),

    /// The configuration parsed but violates one or more startup rules. Carries every violation
    /// found.
    #[error("configuration is invalid: {} problem(s)", .0.len())]
    Invalid(Vec<Violation>),
}

impl ConfigError {
    /// The operator-facing report, written to stderr before any subscriber exists.
    /// One block per problem, stable order, no supplied values.
    #[must_use]
    pub fn report(&self, role: RuntimeRole) -> String {
        match self {
            Self::Source(error) => validate::report_unreadable(role, error),
            Self::Invalid(violations) => validate::report_invalid(role, violations),
        }
    }

    /// `78` — `EX_CONFIG` from `sysexits.h`. `systemctl status` and `systemd-analyze` render it
    /// as `EXIT_CONFIG`, which is what distinguishes "your configuration is wrong" from "the
    /// process crashed" in a unit that is restarting every ten seconds. Each unit runs
    /// `check-config` as a pre-start step, so it is normally that step which carries this code.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        78
    }
}

#[cfg(test)]
mod access_owner_tests {
    #![allow(
        clippy::expect_used,
        clippy::panic,
        reason = "assertions in a test module"
    )]
    #![allow(
        clippy::result_large_err,
        reason = "as the integration suites: figment::Error is the specified payload"
    )]

    use figment::Jail;

    use super::{RuntimeRole, figment, load_from};
    use crate::config::{ConfigError, TelegramConfig};

    /// The environment a webhook-role load needs besides the value under test, so an assertion is
    /// about that value alone.
    fn admit_webhook_basics(jail: &mut Jail) {
        jail.set_env(
            "RATATOSKR__BOT_API__TOKEN",
            "123456:TEST-owner-config-token",
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

    #[test]
    fn access_owner_telegram_user_id_parses_positive_i64() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            admit_webhook_basics(jail);
            jail.set_env("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "700100200");
            let config = load_from(RuntimeRole::Webhook, figment(RuntimeRole::Webhook))
                .expect("a positive id parses");
            assert_eq!(config.access.owner_telegram_user_id, Some(700_100_200));
            Ok(())
        });
    }

    #[test]
    fn access_owner_telegram_user_id_refuses_zero_negative_non_integer() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            admit_webhook_basics(jail);

            // Zero and a negative parse as i64 but are refused by rule V14: Telegram user ids
            // are positive. The refusal names the key and never echoes the value.
            for bad in ["0", "-700100200"] {
                jail.set_env("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", bad);
                let error = load_from(RuntimeRole::Webhook, figment(RuntimeRole::Webhook))
                    .expect_err("a non-positive owner id must be refused");
                let ConfigError::Invalid(violations) = &error else {
                    panic!("expected a V14 violation for a non-positive id, got {error:?}");
                };
                let violation = violations
                    .iter()
                    .find(|violation| violation.key == "access.owner_telegram_user_id")
                    .expect("the V14 violation to name the key");
                assert_eq!(
                    violation.env_var,
                    "RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID"
                );
                assert!(
                    violation.rule.contains("positive"),
                    "the rule must say what positive means: {:?}",
                    violation.rule
                );
            }

            // A non-integer cannot even be extracted; the report names the key and no value.
            jail.set_env("RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID", "not-a-number");
            let error = load_from(RuntimeRole::Webhook, figment(RuntimeRole::Webhook))
                .expect_err("a non-integer owner id must not parse");
            let report = error.report(RuntimeRole::Webhook);
            assert!(report.contains("access.owner_telegram_user_id"), "{report}");
            assert!(!report.contains("not-a-number"), "{report}");
            Ok(())
        });
    }

    /// The defaults carry no owner: one names a real principal and cannot be fabricated. Kept
    /// next to the parse tests because both pin [`AccessConfig`]'s contract inside
    /// [`TelegramConfig`].
    #[test]
    fn the_access_section_defaults_to_no_owner() {
        let config = TelegramConfig::defaults(RuntimeRole::Dispatcher);
        assert_eq!(config.access.owner_telegram_user_id, None);
    }
}
