//! A session exchange binds the Telegram identity, and a bound user receives notifications
//! (XR-021 CONTRACTS S11). Its own test binary because it owns the process-global metrics
//! recorder, as the metrics test in `notifications.rs` does for its binary.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::routing::post;
use platform_api::assertion::AssertionIssuer;
use platform_api::session::{SessionSource, SystemClock};
use serde_json::json;
use telegram_core::RuntimeRole;
use telegram_persistence::IdentityProfile;
use telegram_persistence::test_support::TestDatabase;

/// The recipient the notification envelope names, and the user the fake Platform mints.
const PLATFORM_USER: &str = "018f0000-0000-7000-8000-000000000005";
const TELEGRAM_USER: i64 = 700_100_200;

async fn exchange_server() -> url::Url {
    let app = axum::Router::new().route(
        "/v1/sessions/telegram",
        post(|| async {
            (
                StatusCode::CREATED,
                Json(json!({
                    "credential": "synthetic-session-credential",
                    "expires_at": "2030-01-01T00:00:00Z",
                    "user_id": PLATFORM_USER,
                })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let bound = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).into_future().await;
    });
    url::Url::parse(&format!("http://{bound}")).expect("url")
}

/// One notification envelope. Each `n` is a distinct event and notification: the inbox remembers
/// every event it admitted, even one that had no eligible chat, so a redelivery of the same event
/// is a duplicate by design and the bound user's notification must be a new one.
fn notification_envelope(class: &str, n: u8) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "event_id": format!("018f0000-0000-7000-8000-0000000006{n:02}"),
        "event_type": "platform.notification.raised.v1",
        "occurred_at": "2026-08-25T12:00:00Z",
        "producer": "ratatoskr-platform",
        "aggregate_id": format!("notification:018f0000-0000-7000-8000-0000000007{n:02}"),
        "correlation_id": "operation:018f0000-0000-7000-8000-000000000302",
        "tenant_id": format!("user:{PLATFORM_USER}"),
        "schema_version": 1,
        "payload": {
            "notification_id": format!("018f0000-0000-7000-8000-0000000007{n:02}"),
            "class_registry_version": 1,
            "class": class,
            "recipient": format!("user:{PLATFORM_USER}"),
            "title": "Safe synthetic notification"
        }
    }))
    .expect("notification envelope")
}

/// A webhook-style exchange through the dispatcher's own installed hook is all it takes: no
/// `update telegram.identities` SQL anywhere. The bound user then receives a notification instead
/// of ending with no eligible chat, and the binding outcomes are counted.
#[tokio::test]
async fn a_notification_reaches_a_user_bound_by_a_session_exchange() {
    let guard = telegram_telemetry::init(
        &telegram_core::config::TelemetryConfig::default(),
        RuntimeRole::Dispatcher,
    )
    .expect("isolated test binary installs telemetry");
    let database = TestDatabase::create().await.expect("database");
    database
        .database
        .ensure_identity(TELEGRAM_USER, &IdentityProfile::default())
        .await
        .expect("the owner identity");
    database
        .database
        .ensure_chat(TELEGRAM_USER)
        .await
        .expect("the private chat");

    let sessions = SessionSource::new(
        platform_api::Client::new(&exchange_server().await, Duration::from_secs(5))
            .expect("platform client"),
        AssertionIssuer::from_seed(&[7u8; 32], "edge").expect("issuer"),
        Box::new(SystemClock),
    )
    .with_on_mint(telegram_http::identity_binding_hook(
        database.database.clone(),
    ));

    // Before any exchange the identity is unbound and the notification has nowhere to go.
    database
        .database
        .bind_private_chat(TELEGRAM_USER, TELEGRAM_USER)
        .await
        .expect("the private chat binding");
    let unbound = ratatoskr_telegram_dispatcher::notifications::process_payload(
        &database.database,
        TELEGRAM_USER,
        Some(1),
        &notification_envelope("analysis_ready", 20),
        1_800_000_000,
    )
    .await;
    assert_eq!(
        unbound,
        ratatoskr_telegram_dispatcher::notifications::TransportDisposition::Ack
    );
    assert_eq!(outbound_jobs(&database).await, 0, "no eligible chat yet");

    sessions
        .credential(&TELEGRAM_USER.to_string())
        .await
        .expect("the exchange binds the identity");
    ratatoskr_telegram_dispatcher::notifications::process_payload(
        &database.database,
        TELEGRAM_USER,
        Some(2),
        &notification_envelope("analysis_ready", 21),
        1_800_000_001,
    )
    .await;
    assert_eq!(
        outbound_jobs(&database).await,
        1,
        "the bound user's notification was enqueued for the private chat"
    );

    // A second network exchange for the same mapping is `unchanged`; a subject that is not a
    // Telegram user id cannot be bound and is `failed`, without failing the exchange.
    let subject = TELEGRAM_USER.to_string();
    assert!(
        sessions
            .invalidate_credential(&subject, "synthetic-session-credential")
            .await
    );
    sessions
        .credential(&subject)
        .await
        .expect("second exchange");
    sessions
        .credential("not-a-telegram-user-id")
        .await
        .expect("an unbindable subject does not fail the exchange");

    let exposition = guard.metrics_handle().render();
    for outcome in ["bound", "unchanged", "failed"] {
        let sample = format!("telegram_identity_binding_total{{outcome=\"{outcome}\"}} 1");
        assert!(
            exposition.contains(&sample),
            "missing {sample}:\n{exposition}"
        );
    }
    database.cleanup().await.expect("cleanup");
}

async fn outbound_jobs(database: &TestDatabase) -> i64 {
    sqlx::query_scalar("select count(*) from telegram.outbound_jobs where chat_id = $1")
        .bind(TELEGRAM_USER)
        .fetch_one(database.pool())
        .await
        .expect("job count")
}
