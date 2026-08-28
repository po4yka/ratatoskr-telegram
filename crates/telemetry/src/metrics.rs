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

/// `telegram_updates_denied_total{class}` — counter. One increment per update the authorization
/// gate refuses, labelled by the closed outcome vocabulary: `unknown_sender`, `disabled_identity`,
/// `disabled_chat`, `non_private_chat`. Deliberately identifier-free: the classes are externally
/// indistinguishable by design, and the labels must not become an enrollment oracle either.
pub const TELEGRAM_UPDATES_DENIED_TOTAL: &str = "telegram_updates_denied_total";

/// `telegram_webhook_duration_seconds` — histogram on [`DURATION_BUCKETS`]. Admission only:
/// verification, limits, parse, dedupe insert and queue handoff — never downstream processing,
/// which happens after the response.
pub const TELEGRAM_WEBHOOK_DURATION_SECONDS: &str = "telegram_webhook_duration_seconds";

/// `telegram_delivery_duration_seconds` — histogram on [`DURATION_BUCKETS`]. The Bot API wire
/// call only: claim, guards, and settlement are queue work, not delivery latency.
pub const TELEGRAM_DELIVERY_DURATION_SECONDS: &str = "telegram_delivery_duration_seconds";

/// `telegram_delivery_retries_total{class}` — counter. One increment per job a retryable failure
/// sends back to the queue, labelled by the closed retry vocabulary: `transient`,
/// `rate_limited`. Dead-lettered transients are counted under
/// [`TELEGRAM_DELIVERY_FAILURES_TOTAL`] as `dead_letter`, not here.
pub const TELEGRAM_DELIVERY_RETRIES_TOTAL: &str = "telegram_delivery_retries_total";
/// Capture submission outcomes, by closed safe class.
pub const TELEGRAM_CAPTURE_SUBMISSIONS_TOTAL: &str = "telegram_capture_submissions_total";
/// Follower lifecycle events, by closed safe class (started/resumed/ended/dropped).
pub const TELEGRAM_OPERATION_FOLLOWS_TOTAL: &str = "telegram_operation_follows_total";

/// `telegram_interaction_token_presentations_total{surface,outcome}` — counter. Presentation of
/// callback or deep-link authority by closed result class only; token and scope values are never
/// labels.
pub const TELEGRAM_INTERACTION_TOKEN_PRESENTATIONS_TOTAL: &str =
    "telegram_interaction_token_presentations_total";

/// `telegram_dialogue_transitions_total{kind,outcome}` — counter. Successful transitions,
/// refusals, and timeout expiry using closed dialogue-kind/outcome vocabularies.
pub const TELEGRAM_DIALOGUE_TRANSITIONS_TOTAL: &str = "telegram_dialogue_transitions_total";

/// `telegram_interaction_cleanup_rows_total{kind}` — counter. Rows expired or removed by bounded
/// worker-owned cleanup passes; no interaction identifier appears in the label set.
pub const TELEGRAM_INTERACTION_CLEANUP_ROWS_TOTAL: &str = "telegram_interaction_cleanup_rows_total";

/// `telegram_rate_limit_waits_total` — counter. One increment per authoritative Telegram `429`
/// pause the sender honours and cools the chat down for.
pub const TELEGRAM_RATE_LIMIT_WAITS_TOTAL: &str = "telegram_rate_limit_waits_total";

/// `telegram_delivery_failures_total{class}` — counter. One increment per dead-lettered job,
/// labelled by the closed permanent vocabulary (`bot_blocked`, `chat_not_found`,
/// `membership_lost`, `message_not_editable`, `edit_target_gone`, `invalid_payload`,
/// `chat_migrated`) plus `dead_letter` for transients that exhausted their attempt bound.
pub const TELEGRAM_DELIVERY_FAILURES_TOTAL: &str = "telegram_delivery_failures_total";

/// `telegram_outbound_queue_depth{state}` — gauge, sampled by the sender loop each cycle.
/// Labels are the schema's own job-state tokens (`ready`, `retry_wait`, `sending`, ...), so the
/// depth of a stuck queue is visible without any identifier in the label set.
pub const TELEGRAM_OUTBOUND_QUEUE_DEPTH: &str = "telegram_outbound_queue_depth";

/// `telegram_projection_events_total{outcome}` — counter. One increment per consumed operation
/// event, labelled by the closed accept vocabulary: `recorded`, `duplicate`, `post_terminal`,
/// `stale`, `unbound`.
pub const TELEGRAM_PROJECTION_EVENTS_TOTAL: &str = "telegram_projection_events_total";

/// `telegram_notification_events_total{outcome,class}` — receipt, policy, queue, and sender
/// outcomes. Both labels are closed: unknown future producer classes collapse to `other`.
pub const TELEGRAM_NOTIFICATION_EVENTS_TOTAL: &str = "telegram_notification_events_total";

/// `telegram_notification_backlog` — pending messages reported by the fixed durable.
pub const TELEGRAM_NOTIFICATION_BACKLOG: &str = "telegram_notification_backlog";

/// `telegram_notification_lag` — undelivered plus explicit-ack-pending messages.
pub const TELEGRAM_NOTIFICATION_LAG: &str = "telegram_notification_lag";

/// Latency buckets, in seconds. Shared by every duration histogram this workspace will emit, so
/// graphs of different subsystems stay comparable.
pub const DURATION_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];
