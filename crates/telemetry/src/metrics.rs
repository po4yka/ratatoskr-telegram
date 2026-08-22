//! Every instrument this workspace emits, and nothing else.
//!
//! Prometheus pull on the admin listener. Metrics are **not** exported over OTLP: an OTLP metrics
//! pipeline discards every recording when no collector is running, so a developer would reasonably
//! think metrics work when they do not, whereas `curl localhost:<admin>/metrics` shows the truth.
//!
//! Cardinality is bounded by construction: every label value is a closed set the compiler or the
//! configuration vocabulary counts, never a request-controlled string.
//!
//! Naming convention: every metric is `telegram_<subsystem>_<measure>[_<unit>]`, and every numeric
//! name carries a unit suffix. Future metric names are deliberately not pre-registered.

/// `telegram_readiness{role}` — gauge, `0` or `1`. The aggregate of the readiness checks
/// `/health/ready` reports, so a scrape can alarm on the same fact a probe sees.
pub const TELEGRAM_READINESS: &str = "telegram_readiness";

/// `telegram_build_info{role,version,git_sha,rust_version}` — gauge, always `1`.
/// The first thing anyone looks at when a deployment misbehaves: what is actually running.
pub const TELEGRAM_BUILD_INFO: &str = "telegram_build_info";

/// Latency buckets, in seconds. Shared by every duration histogram this workspace will emit, so
/// graphs of different subsystems stay comparable.
pub const DURATION_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];
