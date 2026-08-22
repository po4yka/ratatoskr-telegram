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

/// `telegram_webhook_requests_total{outcome}` — counter. One increment per delivered request,
/// labelled by the closed admission vocabulary: `accepted`, `deduplicated`, `unauthorized`,
/// `too_large`, `wrong_media_type`, `method_not_allowed`, `malformed`, `overloaded`.
pub const TELEGRAM_WEBHOOK_REQUESTS_TOTAL: &str = "telegram_webhook_requests_total";

/// `telegram_updates_received_total{update_kind}` — counter. One increment per delivery whose
/// envelope parsed, whatever admission decided afterwards; unknown kinds collapse to `other`, so
/// the label stays bounded by the update taxonomy, never by request content.
pub const TELEGRAM_UPDATES_RECEIVED_TOTAL: &str = "telegram_updates_received_total";

/// `telegram_webhook_duration_seconds` — histogram on [`DURATION_BUCKETS`]. Admission only:
/// verification, limits, parse, dedupe insert and queue handoff — never downstream processing,
/// which happens after the response.
pub const TELEGRAM_WEBHOOK_DURATION_SECONDS: &str = "telegram_webhook_duration_seconds";

/// Latency buckets, in seconds. Shared by every duration histogram this workspace will emit, so
/// graphs of different subsystems stay comparable.
pub const DURATION_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];
