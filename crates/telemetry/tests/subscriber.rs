//! The subscriber, and the trace ids it mints without any collector.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::OnceLock;

use telegram_core::RuntimeRole;
use telegram_core::config::TelemetryConfig;
use telegram_telemetry::{TelemetryError, TelemetryGuard, init};

/// The one telemetry installation this process gets: a global subscriber and a global metrics
/// recorder can each be installed exactly once, and every test in this file shares one process.
fn installed() -> &'static TelemetryGuard {
    static GUARD: OnceLock<TelemetryGuard> = OnceLock::new();
    GUARD.get_or_init(|| {
        let config = TelemetryConfig::default();
        assert!(
            config.otlp.is_none(),
            "the default configuration must not export spans"
        );
        init(&config, RuntimeRole::Webhook).expect("telemetry must install")
    })
}

/// An invalid log filter is the caller's (configuration's) problem, reported as such — not a
/// failure inside subscriber setup where nothing could report it.
#[test]
fn an_invalid_log_filter_is_a_filter_error() {
    let config = TelemetryConfig {
        log_filter: "info,===not-a-directive===".to_owned(),
        ..TelemetryConfig::default()
    };
    let error = init(&config, RuntimeRole::Webhook).expect_err("a bad filter must fail");
    assert!(matches!(error, TelemetryError::Filter(_)), "{error:?}");
}

/// Installing twice fails as already-installed rather than silently replacing the subscriber. This
/// test tolerates running after the one that installed first; either order fails here.
#[test]
fn installing_twice_fails_as_already_installed() {
    let _installed = installed();
    let second = init(&TelemetryConfig::default(), RuntimeRole::Webhook);
    assert!(
        matches!(second, Err(TelemetryError::AlreadyInstalled)),
        "{second:?}",
    );
}

/// An exporterless tracer provider still mints valid, sampled, non-zero W3C trace ids, which is
/// what puts a real `trace_id` in every log line before any collector exists.
#[test]
fn spans_carry_non_zero_trace_ids_with_no_collector() {
    let _installed = installed();

    let span = tracing::info_span!("telegram.test");
    let trace_id =
        telegram_telemetry::correlation::trace_id_of(&span).expect("a real W3C trace id");
    span.in_scope(|| {});

    let rendered = trace_id.to_string();
    assert_eq!(rendered.len(), 32);
    assert!(
        rendered
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "a W3C trace id is 32 lowercase hex characters, got {rendered}",
    );
    assert_ne!(
        rendered,
        "0".repeat(32),
        "an all-zero id means the context was invalid",
    );
}
