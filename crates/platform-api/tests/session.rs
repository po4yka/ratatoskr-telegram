//! The per-sender session cache: one exchange per lifetime, refresh before expiry, and a single
//! exchange under concurrency.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use jiff::Timestamp;
use platform_api::session::{Clock, SessionSource};
use platform_api::{Client, assertion::AssertionIssuer};
use serde_json::json;
use url::Url;

const SUBJECT: &str = "900700601";
const AUDIENCE: &str = "ratatoskr-edge";
const START_SECS: i64 = 1_786_960_800; // 2026-08-17T10:00:00Z

/// A frozen clock the test moves by hand. Cheap to clone so one instance backs both the test
/// and the source under test.
#[derive(Clone)]
struct FakeClock(Arc<AtomicI64>);

impl FakeClock {
    fn starting_now() -> Self {
        Self(Arc::new(AtomicI64::new(START_SECS)))
    }

    fn set_to(&self, epoch_secs: i64) {
        self.0.store(epoch_secs, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_second(self.0.load(Ordering::SeqCst)).expect("test instant")
    }
}

fn issuer() -> AssertionIssuer {
    let seed = [7u8; 32];
    AssertionIssuer::from_seed(&seed, AUDIENCE).expect("issuer builds")
}

/// Serves the exchange route; the Nth minted session expires twelve hours after start, so every
/// exchange genuinely extends the sender's freshness and later phases can go stale on purpose.
async fn exchange_harness() -> (Url, Arc<AtomicI64>) {
    let counter = Arc::new(AtomicI64::new(0));
    let app = axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            post(|State(state): State<Arc<AtomicI64>>| async move {
                let issued = state.fetch_add(1, Ordering::SeqCst) + 1;
                let expires_at = Timestamp::from_second(START_SECS + 12 * 3_600 * issued)
                    .expect("test instant")
                    .to_string();
                (
                    StatusCode::CREATED,
                    Json(json!({
                        "credential": format!("synthetic-session-credential-{issued}"),
                        "expires_at": expires_at,
                        "user_id": "018f0000-0000-7000-8000-00000000abcd",
                    })),
                )
            }),
        )
        .with_state(Arc::clone(&counter));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let bound = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("harness runtime");
        let _ = runtime.block_on(axum::serve(listener, app).into_future());
    });
    (
        Url::parse(&format!("http://{bound}")).expect("url"),
        counter,
    )
}

/// Two calls inside the lifetime share one exchange; crossing the refresh margin forces a fresh
/// one; two concurrent stale callers still share that single re-exchange.
#[tokio::test]
async fn sessions_are_exchanged_once_and_refreshed_before_expiry() {
    let (base_url, exchanges) = exchange_harness().await;
    let clock = FakeClock::starting_now();
    let source = SessionSource::new(
        Client::new(&base_url, Duration::from_secs(5)).expect("client"),
        issuer(),
        Box::new(clock.clone()),
    );

    // Inside the first lifetime: one exchange serves both callers.
    let first = source.credential(SUBJECT).await.expect("first resolves");
    let second = source.credential(SUBJECT).await.expect("second resolves");
    assert_eq!(exchanges.load(Ordering::SeqCst), 1, "one exchange so far");
    assert_eq!(first, second);

    // Long past the first session's twelve-hour lifetime: exactly one re-exchange.
    clock.set_to(START_SECS + 13 * 3_600);
    let refreshed = source.credential(SUBJECT).await.expect("refresh resolves");
    assert_eq!(
        exchanges.load(Ordering::SeqCst),
        2,
        "crossing expiry must re-exchange exactly once more"
    );
    assert_ne!(refreshed, first, "a new exchange mints a new credential");

    // Two concurrent stale callers share one in-flight exchange instead of racing nonces.
    clock.set_to(START_SECS + 25 * 3_600);
    let before = exchanges.load(Ordering::SeqCst);
    let (third, fourth) = tokio::join!(source.credential(SUBJECT), source.credential(SUBJECT));
    third.expect("concurrent left");
    fourth.expect("concurrent right");
    assert_eq!(
        exchanges.load(Ordering::SeqCst),
        before + 1,
        "concurrent callers must share one exchange"
    );
}
