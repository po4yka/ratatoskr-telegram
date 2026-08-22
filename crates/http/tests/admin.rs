//! The operator plane, over an in-process router — the four endpoints and their states.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use telegram_core::RuntimeRole;
use telegram_http::RuntimeState;
use telegram_telemetry::identity;
use tower::ServiceExt as _;

/// The admin router of a process in `state`, ready to answer one request.
fn router(state: Arc<RuntimeState>) -> Router {
    telegram_http::admin_router(state, || "# help telegram_build_info\n".to_owned())
}

/// Sends `request` to `router` and returns `(status, headers, body)`.
async fn answer(router: Router, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let response = router
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("a well-formed probe"),
        )
        .await
        .expect("the router answers");
    let status = response.status();
    let headers = response.headers().clone();
    let body = String::from_utf8_lossy(
        &response
            .into_body()
            .collect()
            .await
            .expect("the body is readable")
            .to_bytes(),
    )
    .into_owned();
    (status, headers, body)
}

/// Liveness answers 200 from the moment it is reachable, before startup completes — including
/// throughout a drain later. The property is `state`, not `status`.
#[tokio::test]
async fn liveness_answers_before_startup_completes() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Webhook));
    let (status, _, body) = answer(router(state), "/health/live").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"state\":\"live\""), "{body}");
    assert!(body.contains("\"role\":\"webhook\""), "{body}");
}

/// Readiness fails with a NAMED check until startup completes, then passes.
#[tokio::test]
async fn readiness_fails_until_startup_completes_then_passes() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Dispatcher));

    let (status, _, body) = answer(router(Arc::clone(&state)), "/health/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("\"state\":\"not_ready\""), "{body}");
    assert!(body.contains("\"name\":\"startup\""), "{body}");
    assert!(body.contains("startup_incomplete"), "{body}");

    state.mark_startup_complete();

    let (status, _, body) = answer(router(state), "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"state\":\"ready\""), "{body}");
}

/// A role without a configured database reports NO database check — not a passing one for something
/// that does not exist, and not a failing one either.
#[tokio::test]
async fn no_database_configured_means_no_database_check() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Webhook));
    state.mark_startup_complete();

    let (_, _, body) = answer(router(state), "/health/ready").await;
    assert!(!body.contains("\"database\""), "{body}");
}

/// A configured database appears as a check reflecting the latest background probe, failing with a
/// reason while it does not answer.
#[tokio::test]
async fn a_configured_database_appears_as_a_check_with_its_latest_state() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Webhook));
    state.mark_database_configured();
    state.set_database_reachable(false);
    // Startup has NOT completed, so the process is unready for two reasons now.

    let (status, _, body) = answer(router(Arc::clone(&state)), "/health/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("\"name\":\"database\""), "{body}");
    assert!(body.contains("dependency_unavailable"), "{body}");

    // The next probe answering flips only that check's state.
    state.set_database_reachable(true);
    let (status, _, body) = answer(router(state), "/health/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE); // startup still incomplete
    assert!(body.contains("\"state\":\"pass\""), "{body}");
    assert!(!body.contains("dependency_unavailable"), "{body}");
}

/// Metrics renders Prometheus text from the installed recorder.
#[tokio::test]
async fn metrics_renders_prometheus_text() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Webhook));
    let (status, headers, body) = answer(router(state), "/metrics").await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        headers["content-type"]
            .to_str()
            .expect("ascii")
            .starts_with("text/plain"),
        "prometheus exposition is text",
    );
    assert!(body.contains("telegram_build_info"), "{body}");
}

/// Version reports the build identity; outside a container build the SHA is `unknown`.
#[tokio::test]
async fn version_reports_the_build_identity() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Dispatcher));
    let (_, _, body) = answer(router(state), "/version").await;

    assert!(body.contains(identity::SERVICE_NAME), "{body}");
    assert!(body.contains("\"role\":\"dispatcher\""), "{body}");
    assert!(body.contains(identity::VERSION), "{body}");
    assert!(body.contains("unknown"), "{body}");
    assert!(body.contains(identity::RUST_VERSION), "{body}");
}

/// Every admin response carries `Cache-Control: no-store` — a cached "ready" is a routing decision
/// made from stale data — including the bare 404 of an unknown path.
#[tokio::test]
async fn every_response_carries_no_store_including_the_bare_404() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Webhook));
    let router = router(state);

    for path in [
        "/health/live",
        "/health/ready",
        "/metrics",
        "/version",
        "/nope",
    ] {
        let (status, headers, _) = answer(router.clone(), path).await;
        assert_eq!(
            headers["cache-control"].to_str().expect("ascii"),
            "no-store",
            "{path}",
        );
        if path == "/nope" {
            assert_eq!(status, StatusCode::NOT_FOUND);
            // Bare: no envelope on the operator plane, just a status.
        }
    }
}
