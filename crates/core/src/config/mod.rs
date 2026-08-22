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
    AdminConfig, DatabaseConfig, LogFormat, OtlpConfig, ShutdownConfig, TelegramConfig,
    TelemetryConfig,
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
            // Absent by default, in every role. A database URL carries a credential, so there is
            // no default that is not either wrong or a secret in the source tree.
            database: None,
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
