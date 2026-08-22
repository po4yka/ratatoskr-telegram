//! The typed configuration tree.
//!
//! One shape for both roles. Role-specific requirements are validation rules
//! (`crate::config::validate`), not separate types.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use secrecy::SecretString;
use url::Url;

/// Everything a Telegram binary must know before it can serve.
///
/// One shape for both roles; role-specific requirements are validation rules, not separate types,
/// so there is one thing to document and one thing to test.
///
/// `Serialize` exists for exactly one reason — it seeds the built-in defaults provider. The secret
/// members are `#[serde(skip_serializing)]`, so a default can never carry a secret and a serialized
/// configuration can never leak one.
///
/// Fields without a consumer are deliberately absent: the bot token, the webhook secret and a
/// public-listener table arrive with the plan item that reads them, so every field here has a test
/// and a default that someone has reasoned about.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramConfig {
    /// The operator listener. Every role binds one.
    pub admin: AdminConfig,

    /// The `PostgreSQL` connection. Optional at this milestone — nothing reads persisted data yet —
    /// so a process configured without one starts, serves its probes, and reports no database
    /// check. That is deliberately not "degraded": no request path touches the database, so
    /// claiming degradation would make readiness lie in the safe direction, which is still a lie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<DatabaseConfig>,

    /// The two phases of a graceful stop.
    pub shutdown: ShutdownConfig,

    /// Logging, filtering and span export.
    pub telemetry: TelemetryConfig,
}

/// The operator plane: `/health/live`, `/health/ready`, `/metrics`, `/version`. Never the public
/// API (`AGENTS.md`: "Keep admin and diagnostic endpoints separate from the public user surface").
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    /// `RATATOSKR__ADMIN__BIND`. Default `127.0.0.1:<role default port>`.
    ///
    /// Loopback by default because `SECURITY.md` says "deny by default". A deployment may set a
    /// bridge-reachable address for its metrics stack; what bounds the exposure there is the host
    /// firewall, not the bind address. An any-address default would silently publish `/metrics` on
    /// a developer's LAN, and one variable in an environment file is a loud, deliberate override.
    pub bind: SocketAddr,
}

/// The `PostgreSQL` connection this service owns. It reaches the `telegram` schema and no other.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// `RATATOSKR__DATABASE__URL`. The whole URL is a secret because a `PostgreSQL` URL carries the
    /// password in its user information, so it can never be `Debug`-printed (rule V6).
    #[serde(default, skip_serializing)]
    pub url: SecretString,

    /// `RATATOSKR__DATABASE__MAX_CONNECTIONS`. 1..=100, default 10.
    ///
    /// A ceiling, not a target. `PostgreSQL`'s own `max_connections` is the real limit and the
    /// Ratatoskr services share it; a pool that can exhaust the server is a self-inflicted outage.
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,

    /// `RATATOSKR__DATABASE__ACQUIRE_TIMEOUT_SECONDS`. 1..=30, default 5.
    ///
    /// Bounded well below any future public request timeout so a saturated pool surfaces as a fast,
    /// truthful failure rather than as a request that times out with no explanation.
    #[serde(default = "default_acquire_timeout_seconds")]
    pub acquire_timeout_seconds: u64,
}

const fn default_max_connections() -> u32 {
    10
}

const fn default_acquire_timeout_seconds() -> u64 {
    5
}

/// The two phases of a graceful stop. They are separate knobs because they answer different
/// questions: how long until whatever routes here notices, and how long a request may take.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownConfig {
    /// `RATATOSKR__SHUTDOWN__DRAIN_SECONDS`. 0..=60, default 5.
    ///
    /// Seconds to keep serving after SIGTERM while readiness already reports 503, so whatever is
    /// routing to this process stops before the listener closes. Zero is legal and means in-flight
    /// requests are the only thing the grace window protects.
    #[serde(default = "default_drain_seconds")]
    pub drain_seconds: u64,

    /// `RATATOSKR__SHUTDOWN__GRACE_SECONDS`. 1..=120, default 25.
    /// Seconds allowed for in-flight work after the listener stops accepting.
    #[serde(default = "default_grace_seconds")]
    pub grace_seconds: u64,
}

/// Logging and span export.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryConfig {
    /// `RATATOSKR__TELEMETRY__LOG_FORMAT`. Default `json`.
    #[serde(default)]
    pub log_format: LogFormat,

    /// `RATATOSKR__TELEMETRY__LOG_FILTER`. A `tracing_subscriber::EnvFilter` directive string.
    /// Default `info,tower_http=info,hyper=warn,h2=warn`. Validated at startup (V1), not at
    /// subscriber construction, so a bad filter is a configuration error on stderr rather than a
    /// failure inside telemetry initialisation.
    #[serde(default = "default_log_filter")]
    pub log_filter: String,

    /// `RATATOSKR__TELEMETRY__OTLP__*`. Absent means no span exporter.
    ///
    /// Absence does NOT mean absent trace ids: an `SdkTracerProvider` with zero span processors
    /// still mints a valid, sampled, non-zero W3C trace id, so `trace_id` is real in every log line
    /// with no collector deployed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otlp: Option<OtlpConfig>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            log_format: LogFormat::default(),
            log_filter: default_log_filter(),
            otlp: None,
        }
    }
}

/// How a log line is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// One JSON object per line. The default, because production log collectors parse it.
    #[default]
    Json,
    /// Human-readable, for `cargo run`.
    Pretty,
}

/// The OTLP span exporter.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OtlpConfig {
    /// `RATATOSKR__TELEMETRY__OTLP__ENDPOINT`, e.g. `https://collector.example:4317`.
    pub endpoint: Url,

    /// `RATATOSKR__TELEMETRY__OTLP__TIMEOUT_SECONDS`. 1..=60, default 5.
    #[serde(default = "default_otlp_timeout_seconds")]
    pub timeout_seconds: u64,

    /// `RATATOSKR__TELEMETRY__OTLP__HEADERS__<NAME>` — collector authentication.
    ///
    /// The only secrets in this tree today. `Debug` renders `[REDACTED]` however deeply nested;
    /// there is no `Display`; `skip_serializing` means they cannot be written out; the value is
    /// zeroized on drop; and `rg expose_secret` enumerates every site that touches the plaintext.
    #[serde(default, skip_serializing)]
    pub headers: BTreeMap<String, SecretString>,
}

/// The default of [`ShutdownConfig::drain_seconds`].
pub(super) fn default_drain_seconds() -> u64 {
    5
}

/// The default of [`ShutdownConfig::grace_seconds`].
pub(super) fn default_grace_seconds() -> u64 {
    25
}

/// The default of [`TelemetryConfig::log_filter`].
pub(super) fn default_log_filter() -> String {
    "info,tower_http=info,hyper=warn,h2=warn".to_owned()
}

/// The default of [`OtlpConfig::timeout_seconds`].
pub(super) fn default_otlp_timeout_seconds() -> u64 {
    5
}
