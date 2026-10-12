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
use platform_api::session::{Clock, OnMint, SessionSource};
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

#[tokio::test]
async fn credential_invalidation_is_compare_and_remove() {
    let (base_url, exchanges) = exchange_harness().await;
    let clock = FakeClock::starting_now();
    let source = SessionSource::new(
        Client::new(&base_url, Duration::from_secs(5)).expect("client"),
        issuer(),
        Box::new(clock.clone()),
    );

    let first = source.credential(SUBJECT).await.expect("first credential");
    clock.set_to(START_SECS + 13 * 3_600);
    let refreshed = source
        .credential(SUBJECT)
        .await
        .expect("refreshed credential");

    assert!(
        !source.invalidate_credential(SUBJECT, &first).await,
        "a rejected old credential cannot delete the concurrent refresh"
    );
    assert_eq!(
        source.credential(SUBJECT).await.expect("cached refresh"),
        refreshed
    );
    assert_eq!(exchanges.load(Ordering::SeqCst), 2);

    assert!(
        source.invalidate_credential(SUBJECT, &refreshed).await,
        "the exact rejected credential is removed"
    );
    let after_rejection = source
        .credential(SUBJECT)
        .await
        .expect("replacement credential");
    assert_ne!(after_rejection, refreshed);
    assert_eq!(exchanges.load(Ordering::SeqCst), 3);
}

/// Two callers that were rejected with the same old credential: the first drops it and exchanges
/// a new one, the second finds the cache already holds a newer credential, so its invalidation
/// removes nothing and no third exchange happens (XR-021 CONTRACTS R4-08 rule 3).
#[tokio::test]
async fn an_invalidation_never_removes_a_newer_credential() {
    let (base_url, exchanges) = exchange_harness().await;
    let source = SessionSource::new(
        Client::new(&base_url, Duration::from_secs(5)).expect("client"),
        issuer(),
        Box::new(FakeClock::starting_now()),
    );

    let rejected = source.credential(SUBJECT).await.expect("first credential");
    assert!(
        source.invalidate_credential(SUBJECT, &rejected).await,
        "the first rejection removes the current credential"
    );
    let replacement = source
        .credential(SUBJECT)
        .await
        .expect("replacement credential");
    assert_ne!(replacement, rejected);

    assert!(
        !source.invalidate_credential(SUBJECT, &rejected).await,
        "the second caller's rejection must not remove the newer credential"
    );
    assert_eq!(
        source.credential(SUBJECT).await.expect("cached"),
        replacement
    );
    assert_eq!(
        exchanges.load(Ordering::SeqCst),
        2,
        "two exchanges in total, none for the stale invalidation"
    );
}

/// The Platform user id the exchange harness always returns.
const PLATFORM_USER: &str = "018f0000-0000-7000-8000-00000000abcd";

type Minted = Arc<std::sync::Mutex<Vec<(String, uuid::Uuid)>>>;

/// A hook that records every call it receives.
fn recording_hook() -> (OnMint, Minted) {
    let minted: Minted = Arc::default();
    let sink = Arc::clone(&minted);
    let hook: OnMint = Arc::new(move |subject, user_id| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            sink.lock().expect("minted lock").push((subject, user_id));
        })
    });
    (hook, minted)
}

/// The hook sees every network exchange exactly once - first mint, refresh, and one shared
/// re-exchange under concurrency - and never a cache hit (XR-021 CONTRACTS S11).
#[tokio::test]
async fn the_on_mint_hook_runs_once_per_network_exchange_and_not_on_cache_hits() {
    let (base_url, exchanges) = exchange_harness().await;
    let clock = FakeClock::starting_now();
    let (hook, minted) = recording_hook();
    let source = SessionSource::new(
        Client::new(&base_url, Duration::from_secs(5)).expect("client"),
        issuer(),
        Box::new(clock.clone()),
    )
    .with_on_mint(hook);

    source.credential(SUBJECT).await.expect("first resolves");
    source.credential(SUBJECT).await.expect("cache hit");
    assert_eq!(exchanges.load(Ordering::SeqCst), 1);
    assert_eq!(
        *minted.lock().expect("minted lock"),
        [(SUBJECT.to_owned(), PLATFORM_USER.parse().expect("uuid"))],
        "one exchange, one hook call carrying the subject and the Platform user"
    );

    clock.set_to(START_SECS + 13 * 3_600);
    source.credential(SUBJECT).await.expect("refresh resolves");
    assert_eq!(exchanges.load(Ordering::SeqCst), 2);
    assert_eq!(minted.lock().expect("minted lock").len(), 2, "the refresh");

    clock.set_to(START_SECS + 25 * 3_600);
    let (left, right) = tokio::join!(source.credential(SUBJECT), source.credential(SUBJECT));
    left.expect("concurrent left");
    right.expect("concurrent right");
    assert_eq!(exchanges.load(Ordering::SeqCst), 3);
    assert_eq!(
        minted.lock().expect("minted lock").len(),
        3,
        "two concurrent callers share one exchange and therefore one hook call"
    );
}
