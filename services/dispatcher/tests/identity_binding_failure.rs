//! A binding write that fails never fails the session exchange and is counted as `failed`
//! (XR-021 CONTRACTS S11). Its own test binary because it owns the process-global metrics
//! recorder, as `identity_binding.rs` does for its binary.

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

const TELEGRAM_USER: i64 = 700_100_201;
const CREDENTIAL: &str = "synthetic-session-credential";

async fn exchange_server() -> url::Url {
    let app = axum::Router::new().route(
        "/v1/sessions/telegram",
        post(|| async {
            (
                StatusCode::CREATED,
                Json(json!({
                    "credential": CREDENTIAL,
                    "expires_at": "2030-01-01T00:00:00Z",
                    "user_id": "018f0000-0000-7000-8000-000000000005",
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

/// The database write itself fails (a trigger rejects every update of an identity row), which is
/// the only way `bind_internal_user` can error once the row exists. The exchange still hands back
/// the credential, the failed write leaves the identity unbound, and the outcome is counted.
#[tokio::test]
async fn a_failing_binding_write_does_not_fail_the_exchange_and_is_counted() {
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
    sqlx::raw_sql(
        "create function telegram.reject_identity_update() returns trigger language plpgsql as \
         $$ begin raise exception 'injected identity write fault'; end $$; \
         create trigger reject_identity_update before update on telegram.identities \
         for each row execute function telegram.reject_identity_update();",
    )
    .execute(database.pool())
    .await
    .expect("fault injection");

    let sessions = SessionSource::new(
        platform_api::Client::new(&exchange_server().await, Duration::from_secs(5))
            .expect("platform client"),
        AssertionIssuer::from_seed(&[7u8; 32], "edge").expect("issuer"),
        Box::new(SystemClock),
    )
    .with_on_mint(telegram_http::identity_binding_hook(
        database.database.clone(),
    ));

    let credential = sessions
        .credential(&TELEGRAM_USER.to_string())
        .await
        .expect("the exchange succeeds although the binding write failed");
    assert_eq!(credential, CREDENTIAL);

    let bound: Option<uuid::Uuid> = sqlx::query_scalar(
        "select internal_user_id from telegram.identities where telegram_user_id = $1",
    )
    .bind(TELEGRAM_USER)
    .fetch_one(database.pool())
    .await
    .expect("the owner identity");
    assert_eq!(
        bound, None,
        "the failed write must leave the identity unbound"
    );

    let exposition = guard.metrics_handle().render();
    assert!(
        exposition.contains("telegram_identity_binding_total{outcome=\"failed\"} 1"),
        "the failed write must be counted:\n{exposition}"
    );
    assert!(
        !exposition.contains("outcome=\"bound\""),
        "nothing was bound:\n{exposition}"
    );
    database.cleanup().await.expect("cleanup");
}
