//! The operation follower against a fake Platform: which bindings get streams, how frames map
//! onto the projection seam, and how reconnects resume.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse as _;
use ratatoskr_telegram_dispatcher::follow::OperationFollower;
use telegram_persistence::test_support::TestDatabase;
use uuid::Uuid;

const AUDIENCE: &str = "ratatoskr-edge-test";
const SEED: [u8; 32] = [
    11u8, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 11, 10, 9, 8, 7,
    6, 5, 4,
];
const OWNER: i64 = 900_700_601;
const BOT: i64 = 700_100_200;
const CHAT: i64 = 900_700_601;

async fn database() -> TestDatabase {
    TestDatabase::create()
        .await
        .expect("a disposable database must be creatable")
}

/// Wait until the harness has served `expected` event-stream opens. `scan_and_follow_once` gives
/// its spawned tasks a fixed beat, which is not a completion signal; under load the streams open
/// later than any fixed delay, so the tests await the observable fact instead.
async fn wait_for_opens(state: &HarnessState, expected: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while state.opens.load(Ordering::SeqCst) < expected {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{expected} stream(s) did not open within ten seconds"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn sessions(base_url: &str) -> Arc<platform_api::session::SessionSource> {
    let client = platform_api::Client::new(
        &url::Url::parse(base_url).expect("harness url"),
        Duration::from_secs(5),
    )
    .expect("client builds");
    let issuer = platform_api::assertion::AssertionIssuer::from_seed(&SEED, AUDIENCE)
        .expect("issuer builds");
    Arc::new(platform_api::session::SessionSource::new(
        client,
        issuer,
        Box::new(platform_api::session::SystemClock),
    ))
}

#[derive(Default)]
struct HarnessState {
    exchanges: AtomicU64,
    opens: AtomicU64,
}

/// One fake Platform serving both routes the follower needs. The events route answers every
/// operation with an accepted frame then a terminal one.
fn platform_harness() -> (String, Arc<HarnessState>) {
    let state = Arc::new(HarnessState::default());
    let shared = Arc::clone(&state);
    let app = axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            axum::routing::post(move |State(state): State<Arc<HarnessState>>| async move {
                state.exchanges.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::CREATED,
                    axum::Json(serde_json::json!({
                        "credential": "synthetic-session-credential",
                        "expires_at": "2030-01-01T00:00:00Z",
                        "user_id": "018f0000-0000-7000-8000-00000000abcd",
                    })),
                )
            }),
        )
        .route(
            "/v1/operations/{operation_id}/events",
            axum::routing::get(move |State(state): State<Arc<HarnessState>>,
                                     _headers: HeaderMap,
                                     AxumPath(_id): AxumPath<String>| async move {
                state.opens.fetch_add(1, Ordering::SeqCst);
                let body = concat!(
                    "id: 018f0000-0000-7000-8000-0000000000a1\n",
                    "event: progress\n",
                    "data: {\"status\":\"accepted\",\"observed_at\":\"2026-08-17T10:00:00Z\"}\n\n",
                    "id: 018f0000-0000-7000-8000-0000000000a2\n",
                    "event: progress\n",
                    "data: {\"status\":\"succeeded\",\"observed_at\":\"2026-08-17T10:00:05Z\"}\n\n",
                );
                (
                    StatusCode::OK,
                    [("content-type", "text/event-stream")],
                    body,
                )
                    .into_response()
            }),
        )
        .with_state(Arc::clone(&shared));
    // Bind inside the serving runtime: a `tokio::net::TcpListener` belongs to the runtime whose
    // driver registered it, so binding here and serving on another thread's runtime hands the
    // accept loop IO it cannot poll ("A Tokio 1.x context ... is being shutdown"). One thread owns
    // bind, serve, and shutdown together; the test only learns the address.
    let (bound_tx, bound_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("rt");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let bound = listener.local_addr().expect("addr");
            bound_tx
                .send(bound)
                .expect("the test receives the bound address");
            let _ = axum::serve(listener, app).into_future().await;
        });
    });
    let bound = bound_rx.recv().expect("the harness binds before returning");
    (format!("http://{bound}"), state)
}

fn follower(base_url: &str, db: &TestDatabase) -> OperationFollower {
    let (feed_tx, feed_rx) = tokio::sync::mpsc::channel(64);
    // Hold the receiver open for the test's life by leaking it into a parked task.
    let (_keep_alive, keep_open) = tokio::sync::mpsc::channel::<()>(1);
    std::mem::forget((feed_tx.clone(), feed_rx, keep_open));
    OperationFollower::new(db.database.clone(), feed_tx, sessions(base_url))
}

async fn seed_live(db: &TestDatabase, operation: Uuid) {
    db.database
        .ensure_operation_binding(BOT, operation, CHAT)
        .await
        .expect("binding");
    db.database
        .issue_operation_intent(
            &telegram_persistence::interaction_tokens::NewOperationIntent {
                scope: telegram_persistence::interaction_tokens::TokenScope {
                    bot_id: BOT,
                    telegram_user_id: OWNER,
                    chat_id: CHAT,
                    message_id: None,
                },
                operation_id: operation,
                payload: telegram_persistence::interaction_tokens::OperationIntentPayload {
                    source_url: Some("https://example.test/a".to_owned()),
                    metadata: None,
                },
                expires_at: 2_000_000_000,
            },
            1_800_000_000,
        )
        .await
        .expect("intent");
}

async fn seed_terminal(db: &TestDatabase, operation: Uuid) {
    seed_live(db, operation).await;
    sqlx::query("update telegram.message_bindings set terminal = true where operation_id = $1")
        .bind(operation)
        .execute(db.pool())
        .await
        .expect("terminal flip");
}

/// After a scan, exactly the live bindings opened streams - the terminal one is never followed -
/// and a second scan over the same set opens nothing new.
#[tokio::test]
async fn non_terminal_bindings_are_followed_once_each_after_restart() {
    let (base_url, state) = platform_harness();
    let db = database().await;
    for tail in 1..=3u16 {
        seed_live(
            &db,
            Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0000 | u128::from(tail)),
        )
        .await;
    }
    seed_terminal(
        &db,
        Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0004),
    )
    .await;

    let follower = follower(&base_url, &db);
    follower.scan_and_follow_once().await;
    wait_for_opens(&state, 3).await;

    assert_eq!(
        state.opens.load(Ordering::SeqCst),
        3,
        "three streams for three live bindings"
    );

    // A second scan adds nothing: the in-flight set suppresses duplicates.
    follower.scan_and_follow_once().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        state.opens.load(Ordering::SeqCst),
        3,
        "a followed operation is not reopened by the next scan"
    );
}

/// Frames map onto the projection seam through the real consumer, and the terminal frame stops
/// the follow: one stream open, two accepted renders, nothing after succeeded.
#[tokio::test]
async fn frames_map_dedupe_and_stop_at_terminal() {
    static GUARD: std::sync::OnceLock<telegram_telemetry::TelemetryGuard> =
        std::sync::OnceLock::new();
    let guard = GUARD.get_or_init(|| {
        telegram_telemetry::init(
            &telegram_core::config::TelemetryConfig::default(),
            telegram_core::RuntimeRole::Dispatcher,
        )
        .expect("the registry installs once per process")
    });

    let (base_url, state) = platform_harness();
    let db = database().await;
    let operation = Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0005);
    seed_live(&db, operation).await;

    let clock: Arc<dyn ratatoskr_telegram_dispatcher::outbound::clock::Clock> =
        Arc::new(ratatoskr_telegram_dispatcher::outbound::clock::SystemClock);
    let consumer = ratatoskr_telegram_dispatcher::projection::ProjectionConsumer::new(
        db.database.clone(),
        clock,
        0,
        None,
    );

    // A real feed drained deterministically: the harness streams exactly one accepted frame and
    // one terminal frame per open, so the test awaits each event through the channel instead of
    // sleeping and hoping the pipeline has finished. No fixed delay survives load.
    let (feed_tx, mut feed_rx) = tokio::sync::mpsc::channel::<
        ratatoskr_telegram_dispatcher::projection::event::OperationEvent,
    >(64);

    let sessions = sessions(&base_url);
    let follower = OperationFollower::new(db.database.clone(), feed_tx, sessions);
    follower.scan_and_follow_once().await;

    for _ in 0..2 {
        let event = feed_rx
            .recv()
            .await
            .expect("the harness streams two frames per open");
        consumer
            .accept(&event)
            .await
            .expect("both frames render without a storage failure");
    }

    // Both frames were consumed, so the stream was opened exactly once.
    assert_eq!(state.opens.load(Ordering::SeqCst), 1);

    // Both distinct frames rendered as edits; the terminal flag is set exactly once.
    let edits: i64 = sqlx::query_scalar(
        "select count(*) from telegram.outbound_jobs where kind = 'edit_message_text'",
    )
    .fetch_one(db.pool())
    .await
    .expect("job count");
    assert_eq!(edits, 2, "accepted + succeeded render once each");
    let terminal: i64 =
        sqlx::query_scalar("select count(*) from telegram.message_bindings where terminal")
            .fetch_one(db.pool())
            .await
            .expect("binding");
    assert_eq!(terminal, 1);

    let exposition = guard.metrics_handle().render();
    let series = "telegram_operation_follows_total{event=\"started\"}";
    assert!(
        exposition.contains(series),
        "{series} missing from:\n{exposition}"
    );
}
