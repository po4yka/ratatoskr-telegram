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
/// Fields without a consumer are deliberately absent: a field arrives with the plan item that reads
/// it, so every field here has a test and a default that someone has reasoned about.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramConfig {
    /// The operator listener. Every role binds one.
    pub admin: AdminConfig,

    /// Who may talk to this deployment. Present for every role; only the webhook role requires
    /// anything of it today (V14).
    #[serde(default)]
    pub access: AccessConfig,

    /// The Bot API endpoint, call budget and bot credential. Present for every role: the values
    /// are defaulted and harmless until a role's validation demands the token.
    #[serde(default)]
    pub bot_api: BotApiConfig,

    /// The `PostgreSQL` connection. Required by the webhook role since it writes through the pool;
    /// required by the dispatcher role since item 4 for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<DatabaseConfig>,

    /// Outbound delivery tuning. Present for every role; only the dispatcher reads it.
    #[serde(default)]
    pub dispatcher: DispatcherConfig,

    /// The Platform endpoint, audience and assertion signing key. Both runtime roles perform
    /// Platform work from this item on, so both read it; validation demands the secrets.
    #[serde(default)]
    pub platform: PlatformConfig,

    /// The public update-intake listener. Webhook-role specific; absent means unconfigured, which
    /// the role requirements refuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookConfig>,

    /// The two phases of a graceful stop.
    pub shutdown: ShutdownConfig,

    /// Logging, filtering and span export.
    pub telemetry: TelemetryConfig,
}

/// Outbound delivery tuning for the dispatcher role: the rate gates, the retry policy, and the
/// render throttle. Every field is validated (V15); the defaults are the small-deployment values.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatcherConfig {
    /// `RATATOSKR__DISPATCHER__GLOBAL_MESSAGES_PER_SECOND`. Sustained global send budget.
    #[serde(default)]
    pub global_messages_per_second: u32,

    /// `RATATOSKR__DISPATCHER__PER_CHAT_MIN_INTERVAL_MS`. Minimum gap between sends to one chat.
    #[serde(default)]
    pub per_chat_min_interval_ms: u64,

    /// `RATATOSKR__DISPATCHER__RENDER_INTERVAL_SECS`. Minimum spacing between eligible edits of
    /// one binding.
    #[serde(default)]
    pub render_interval_secs: u64,

    /// `RATATOSKR__DISPATCHER__MAX_ATTEMPTS`. Claims before a job dead-letters.
    #[serde(default)]
    pub max_attempts: u32,

    /// `RATATOSKR__DISPATCHER__BACKOFF_BASE_SECS`. First transient backoff.
    #[serde(default)]
    pub backoff_base_secs: u32,

    /// `RATATOSKR__DISPATCHER__BACKOFF_CAP_SECS`. Transient backoff ceiling.
    #[serde(default)]
    pub backoff_cap_secs: u32,

    /// `RATATOSKR__DISPATCHER__JITTER_FRACTION_MILLI`. Jitter share of a computed delay,
    /// thousandths (`200` = 20%).
    #[serde(default)]
    pub jitter_fraction_milli: u32,

    /// `RATATOSKR__DISPATCHER__LEASE_TTL_SECS`. How long a claim's lease runs.
    #[serde(default)]
    pub lease_ttl_secs: u32,

    /// `RATATOSKR__DISPATCHER__POLL_IDLE_MS`. Idle poll interval of the sender loop.
    #[serde(default)]
    pub poll_idle_ms: u64,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        // The small-deployment values: 25 msg/s is under Telegram's documented sustained global
        // ceiling with headroom, 1200 ms per chat keeps progress edits quiet, a 4 s render
        // interval coalesces bursts, and 5 attempts with 2..=300 s capped backoff dead-letters a
        // dead target inside minutes, not hours.
        Self {
            global_messages_per_second: 25,
            per_chat_min_interval_ms: 1_200,
            render_interval_secs: 4,
            max_attempts: 5,
            backoff_base_secs: 2,
            backoff_cap_secs: 300,
            jitter_fraction_milli: 200,
            lease_ttl_secs: 60,
            poll_idle_ms: 1_000,
        }
    }
}

/// The Bot API endpoint this service calls, its call budget, and the credential it calls with.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotApiConfig {
    /// `RATATOSKR__BOT_API__BASE_URL`. Default `https://api.telegram.org`.
    ///
    /// Configurable so tests and local runs point at a harness server instead of Telegram; rule V9
    /// keeps a plain-`http` endpoint on loopback only.
    #[serde(default = "default_bot_api_base_url")]
    pub base_url: Url,

    /// `RATATOSKR__BOT_API__TIMEOUT_SECONDS`. 1..=60, default 10.
    ///
    /// The whole-call budget: a Bot API call that outlives its caller's request is a hang with
    /// extra steps. Bounded well under the webhook's acknowledgment expectations.
    #[serde(default = "default_bot_api_timeout_seconds")]
    pub timeout_seconds: u64,

    /// `RATATOSKR__BOT_API__TOKEN`. The bot credential. SECRET.
    ///
    /// Empty default: there is no value that is not either wrong or a secret in the source tree,
    /// and the role requirements refuse an empty token where one is needed (V13).
    #[serde(default, skip_serializing)]
    pub token: SecretString,

    /// `RATATOSKR__BOT_API__USERNAME`. The serving bot's `t.me` handle, without the @. Optional:
    /// the deep-link composer needs it, and a deployment that omits it simply gets text-only
    /// terminal renders rather than broken links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

impl Default for BotApiConfig {
    fn default() -> Self {
        Self {
            base_url: default_bot_api_base_url(),
            timeout_seconds: default_bot_api_timeout_seconds(),
            token: SecretString::default(),
            username: None,
        }
    }
}

/// The Platform public-API endpoint, the assertion audience, and this service's signing half.
///
/// Defaults are development-harness values; both roles' validation (V17) demands the audience and
/// a usable signing key so an unconfigured process refuses before binding anything.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformConfig {
    /// `RATATOSKR__PLATFORM__BASE_URL`. Default `http://127.0.0.1:9463`, the development harness.
    ///
    /// Rule V9's loopback-only plain-`http` allowance applies here exactly as to the Bot API.
    #[serde(default = "default_platform_base_url")]
    pub base_url: Url,

    /// `RATATOSKR__PLATFORM__TIMEOUT_SECONDS`. 1..=60, default 10. Whole-call budget.
    #[serde(default = "default_platform_timeout_seconds")]
    pub timeout_seconds: u64,

    /// `RATATOSKR__PLATFORM__AUDIENCE`. The listener identity assertions may be redeemed at.
    /// SECRET-free but deployment-specific; empty default is refused where required.
    #[serde(default)]
    pub audience: String,

    /// `RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY`. The Ed25519 seed as 64 hex characters.
    /// SECRET. Empty default refused where required; never rendered by check-config.
    #[serde(default, skip_serializing)]
    pub assertion_signing_key: SecretString,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            base_url: default_platform_base_url(),
            timeout_seconds: default_platform_timeout_seconds(),
            audience: String::default(),
            assertion_signing_key: SecretString::default(),
        }
    }
}

fn default_platform_base_url() -> Url {
    // The site exception is the honest spelling for parsing a compile-time literal.
    #[expect(
        clippy::expect_used,
        reason = "a compile-time constant URL cannot fail to parse"
    )]
    Url::parse("http://127.0.0.1:9463").expect("the documented default parses")
}

const fn default_platform_timeout_seconds() -> u64 {
    10
}

/// The public update-intake listener and everything admission needs.
///
/// Present only when configured; the webhook role's validation refuses absence (V13), every other
/// role ignores it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// `RATATOSKR__WEBHOOK__BIND`. Default `127.0.0.1:9469`, continuing this service's allocation
    /// block behind the admin ports.
    ///
    /// Loopback by default (`SECURITY.md`: deny by default). A deployment sets its ingress-facing
    /// address explicitly; what terminates TLS in front of it is the deployment's trusted path.
    #[serde(default = "default_webhook_bind")]
    pub bind: SocketAddr,

    /// `RATATOSKR__WEBHOOK__SECRET_TOKEN`. The value Telegram echoes in
    /// `X-Telegram-Bot-Api-Secret-Token` on every delivery. SECRET.
    ///
    /// Rule V11 bounds it to 16..=256 characters over `[A-Za-z0-9_-]` — Telegram's charset for the
    /// value, with the floor forcing entropy above anything guessable.
    #[serde(default, skip_serializing)]
    pub secret_token: SecretString,

    /// `RATATOSKR__WEBHOOK__MAX_BODY_BYTES`. 1024..=1048576, default 262144.
    ///
    /// An admission ceiling, not a target: real updates are small, and the cap exists so a forged
    /// delivery cannot buy memory with bytes.
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: u32,
}

const fn default_max_body_bytes() -> u32 {
    262_144
}

fn default_webhook_bind() -> SocketAddr {
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 9469)
}

fn default_bot_api_base_url() -> Url {
    // The site exception is the honest spelling for parsing a compile-time literal.
    #[expect(
        clippy::expect_used,
        reason = "a compile-time constant URL cannot fail to parse"
    )]
    Url::parse("https://api.telegram.org").expect("constant URL")
}

const fn default_bot_api_timeout_seconds() -> u64 {
    10
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

/// Who may talk to this deployment: the owner-first access policy seed.
///
/// One member today; more arrives with the plan items that read it, never before.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessConfig {
    /// `RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID`. The deployment owner's Telegram user id.
    ///
    /// A positive i64 (rule V14 refuses every other value). The webhook role requires it: it
    /// resolves every delivery against this id and seeds its identity row at startup. Absent
    /// means unconfigured, which that role's validation refuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_telegram_user_id: Option<i64>,
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
