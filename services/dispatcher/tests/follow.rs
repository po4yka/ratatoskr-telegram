//! The operation follower against a fake Platform: which bindings get streams, how frames map
//! onto the projection seam, and how reconnects resume.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse as _;
use jiff::Timestamp;
use platform_api::session::Clock;
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
const START_SECS: i64 = 1_788_134_400; // 2026-08-29T00:00:00Z

async fn database() -> TestDatabase {
    TestDatabase::create()
        .await
        .expect("a disposable database must be creatable")
}

/// Wait until the harness has served `expected` event-stream opens.
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
    sessions_with_clock(base_url, Box::new(platform_api::session::SystemClock))
}

fn sessions_with_clock(
    base_url: &str,
    clock: Box<dyn Clock>,
) -> Arc<platform_api::session::SessionSource> {
    let client = platform_api::Client::new(
        &url::Url::parse(base_url).expect("harness url"),
        Duration::from_secs(5),
    )
    .expect("client builds");
    let issuer = platform_api::assertion::AssertionIssuer::from_seed(&SEED, AUDIENCE)
        .expect("issuer builds");
    Arc::new(platform_api::session::SessionSource::new(
        client, issuer, clock,
    ))
}

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

fn session_response(credential: &str, expires_at: &str) -> axum::response::Response {
    (
        StatusCode::CREATED,
        axum::Json(serde_json::json!({
            "credential": credential,
            "expires_at": expires_at,
            "user_id": "018f0000-0000-7000-8000-00000000abcd",
        })),
    )
        .into_response()
}

#[derive(Default)]
struct HarnessState {
    exchanges: AtomicU64,
    opens: AtomicU64,
    exchange_attempted: tokio::sync::Notify,
    open_attempted: tokio::sync::Notify,
}

fn transient_exchange_harness() -> (String, Arc<HarnessState>) {
    let state = Arc::new(HarnessState::default());
    let app = axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            axum::routing::post(|State(state): State<Arc<HarnessState>>| async move {
                let exchange = state.exchanges.fetch_add(1, Ordering::SeqCst) + 1;
                state.exchange_attempted.notify_waiters();
                if exchange == 1 {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                session_response(
                    "recovered-session-credential",
                    "2030-01-01T00:00:00Z",
                )
            }),
        )
        .route(
            "/v1/operations/{operation_id}/events",
            axum::routing::get(
                |State(state): State<Arc<HarnessState>>,
                 AxumPath(_id): AxumPath<String>| async move {
                    state.opens.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        [("content-type", "text/event-stream")],
                        concat!(
                            "id: 018f0000-0000-7000-8000-0000000000a3\n",
                            "event: progress\n",
                            "data: {\"status\":\"succeeded\",\"observed_at\":\"2026-08-29T00:00:00Z\"}\n\n",
                        ),
                    )
                },
            ),
        )
        .with_state(Arc::clone(&state));
    (serve_harness(app), state)
}

fn clean_close_harness() -> (String, Arc<HarnessState>) {
    let state = Arc::new(HarnessState::default());
    let app = axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            axum::routing::post(|State(state): State<Arc<HarnessState>>| async move {
                state.exchanges.fetch_add(1, Ordering::SeqCst);
                session_response(
                    "clean-close-session",
                    "2030-01-01T00:00:00Z",
                )
            }),
        )
        .route(
            "/v1/operations/{operation_id}/events",
            axum::routing::get(
                |State(state): State<Arc<HarnessState>>,
                 AxumPath(_id): AxumPath<String>| async move {
                    state.opens.fetch_add(1, Ordering::SeqCst);
                    state.open_attempted.notify_waiters();
                    (
                        StatusCode::OK,
                        [("content-type", "text/event-stream")],
                        "",
                    )
                },
            ),
        )
        .with_state(Arc::clone(&state));
    (serve_harness(app), state)
}

struct CredentialHarnessState {
    exchanges: AtomicU64,
    opens: AtomicU64,
    clock: FakeClock,
    presented: std::sync::Mutex<Vec<String>>,
    open_attempted: tokio::sync::Notify,
}

impl CredentialHarnessState {
    fn new(clock: FakeClock) -> Self {
        Self {
            exchanges: AtomicU64::new(0),
            opens: AtomicU64::new(0),
            clock,
            presented: std::sync::Mutex::new(Vec::new()),
            open_attempted: tokio::sync::Notify::new(),
        }
    }
}

fn refreshed_credential_harness(clock: FakeClock) -> (String, Arc<CredentialHarnessState>) {
    let state = Arc::new(CredentialHarnessState::new(clock));
    let app = axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            axum::routing::post(|State(state): State<Arc<CredentialHarnessState>>| async move {
                let exchange = state.exchanges.fetch_add(1, Ordering::SeqCst) + 1;
                let expires_at = Timestamp::from_second(
                    START_SECS + i64::try_from(exchange).expect("small count") * 3_600,
                )
                .expect("test instant")
                .to_string();
                session_response(&format!("session-{exchange}"), &expires_at)
            }),
        )
        .route(
            "/v1/operations/{operation_id}/events",
            axum::routing::get(
                |State(state): State<Arc<CredentialHarnessState>>,
                 headers: HeaderMap,
                 AxumPath(_id): AxumPath<String>| async move {
                    let open = state.opens.fetch_add(1, Ordering::SeqCst) + 1;
                    let presented = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("missing")
                        .to_owned();
                    state.presented.lock().expect("credentials lock").push(presented);
                    if open == 1 {
                        state.clock.set_to(START_SECS + 4_000);
                    }
                    state.open_attempted.notify_waiters();
                    if open == 1 {
                        (
                            StatusCode::OK,
                            [("content-type", "text/event-stream")],
                            "",
                        )
                            .into_response()
                    } else {
                        (
                            StatusCode::OK,
                            [("content-type", "text/event-stream")],
                            concat!(
                                "id: 018f0000-0000-7000-8000-0000000000a4\n",
                                "event: progress\n",
                                "data: {\"status\":\"succeeded\",\"observed_at\":\"2026-08-29T00:00:00Z\"}\n\n",
                            ),
                        )
                            .into_response()
                    }
                },
            ),
        )
        .with_state(Arc::clone(&state));
    (serve_harness(app), state)
}

struct AuthenticationHarnessState {
    exchanges: AtomicU64,
    clock: FakeClock,
    presented: std::sync::Mutex<Vec<String>>,
    second_rejection_started: tokio::sync::Notify,
    release_second_rejection: tokio::sync::Notify,
    accepted: tokio::sync::Notify,
}

impl AuthenticationHarnessState {
    fn new(clock: FakeClock) -> Self {
        Self {
            exchanges: AtomicU64::new(0),
            clock,
            presented: std::sync::Mutex::new(Vec::new()),
            second_rejection_started: tokio::sync::Notify::new(),
            release_second_rejection: tokio::sync::Notify::new(),
            accepted: tokio::sync::Notify::new(),
        }
    }
}

fn authentication_rejection_harness(clock: FakeClock) -> (String, Arc<AuthenticationHarnessState>) {
    let state = Arc::new(AuthenticationHarnessState::new(clock));
    let app = axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            axum::routing::post(
                |State(state): State<Arc<AuthenticationHarnessState>>| async move {
                    let exchange = state.exchanges.fetch_add(1, Ordering::SeqCst) + 1;
                    let expires_at = Timestamp::from_second(
                        START_SECS + i64::try_from(exchange).expect("small count") * 3_600,
                    )
                    .expect("test instant")
                    .to_string();
                    session_response(&format!("session-{exchange}"), &expires_at)
                },
            ),
        )
        .route(
            "/v1/operations/{operation_id}/events",
            axum::routing::get(
                |State(state): State<Arc<AuthenticationHarnessState>>,
                 headers: HeaderMap,
                 AxumPath(_id): AxumPath<String>| async move {
                    let presented = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("missing")
                        .to_owned();
                    state
                        .presented
                        .lock()
                        .expect("credentials lock")
                        .push(presented.clone());
                    match presented.as_str() {
                        "Bearer session-1" => StatusCode::UNAUTHORIZED.into_response(),
                        "Bearer session-2" => {
                            state.second_rejection_started.notify_waiters();
                            state.release_second_rejection.notified().await;
                            StatusCode::UNAUTHORIZED.into_response()
                        }
                        _ => {
                            state.accepted.notify_waiters();
                            (
                                StatusCode::OK,
                                [("content-type", "text/event-stream")],
                                concat!(
                                    "id: 018f0000-0000-7000-8000-0000000000a5\n",
                                    "event: progress\n",
                                    "data: {\"status\":\"succeeded\",\"observed_at\":\"2026-08-29T00:00:00Z\"}\n\n",
                                ),
                            )
                                .into_response()
                        }
                    }
                },
            ),
        )
        .with_state(Arc::clone(&state));
    (serve_harness(app), state)
}

#[derive(Default)]
struct CancellationHarnessState {
    exchanges: AtomicU64,
    opens: AtomicU64,
    open_attempted: tokio::sync::Notify,
    release_first: tokio::sync::Notify,
}

fn cancellation_harness() -> (String, Arc<CancellationHarnessState>) {
    let state = Arc::new(CancellationHarnessState::default());
    let app = axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            axum::routing::post(
                |State(state): State<Arc<CancellationHarnessState>>| async move {
                    let exchange = state.exchanges.fetch_add(1, Ordering::SeqCst) + 1;
                    session_response(
                        &format!("cancellation-session-{exchange}"),
                        "2030-01-01T00:00:00Z",
                    )
                },
            ),
        )
        .route(
            "/v1/operations/{operation_id}/events",
            axum::routing::get(
                |State(state): State<Arc<CancellationHarnessState>>,
                 AxumPath(_id): AxumPath<String>| async move {
                    let open = state.opens.fetch_add(1, Ordering::SeqCst) + 1;
                    state.open_attempted.notify_waiters();
                    if open == 1 {
                        state.release_first.notified().await;
                        (
                            StatusCode::OK,
                            [("content-type", "text/event-stream")],
                            "",
                        )
                            .into_response()
                    } else {
                        (
                            StatusCode::OK,
                            [("content-type", "text/event-stream")],
                            concat!(
                                "id: 018f0000-0000-7000-8000-0000000000a6\n",
                                "event: progress\n",
                                "data: {\"status\":\"succeeded\",\"observed_at\":\"2026-08-29T00:00:00Z\"}\n\n",
                            ),
                        )
                            .into_response()
                    }
                },
            ),
        )
        .with_state(Arc::clone(&state));
    (serve_harness(app), state)
}

fn serve_harness(app: axum::Router) -> String {
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
            bound_tx
                .send(listener.local_addr().expect("addr"))
                .expect("the test receives the bound address");
            let _ = axum::serve(listener, app).into_future().await;
        });
    });
    let bound = bound_rx.recv().expect("the harness binds before returning");
    format!("http://{bound}")
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
    follower_with_sessions(db, sessions(base_url))
}

fn follower_with_sessions(
    db: &TestDatabase,
    sessions: Arc<platform_api::session::SessionSource>,
) -> OperationFollower {
    let (feed_tx, feed_rx) = tokio::sync::mpsc::channel(64);
    let (_keep_alive, keep_open) = tokio::sync::mpsc::channel::<()>(1);
    std::mem::forget((feed_tx.clone(), feed_rx, keep_open));
    OperationFollower::new(db.database.clone(), feed_tx, sessions)
}

fn run_follower(
    follower: OperationFollower,
) -> (
    tokio::sync::watch::Sender<bool>,
    tokio::task::JoinHandle<()>,
) {
    let (shutdown, cancelled) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(follower.run_until_shutdown(
        cancelled,
        Arc::new(tokio::sync::RwLock::new(())),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ));
    (shutdown, task)
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

#[tokio::test]
async fn temporary_session_exchange_failure_is_retried_while_binding_is_live() {
    let (base_url, state) = transient_exchange_harness();
    let db = database().await;
    let operation = Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0010);
    seed_live(&db, operation).await;

    let first_exchange = state.exchange_attempted.notified();
    let (shutdown, task) = run_follower(follower(&base_url, &db));
    tokio::time::timeout(Duration::from_secs(1), first_exchange)
        .await
        .expect("the first session exchange is attempted");
    let second_exchange = state.exchange_attempted.notified();
    let retried = tokio::time::timeout(Duration::from_secs(7), second_exchange)
        .await
        .is_ok();
    assert!(retried, "a later scan must retry the transient exchange");
    wait_for_opens(&state, 1).await;

    shutdown.send_replace(true);
    task.await.expect("the follower joins");

    assert_eq!(
        state.exchanges.load(Ordering::SeqCst),
        2,
        "the later scan must retry session exchange for a still-live binding"
    );
    assert_eq!(
        state.opens.load(Ordering::SeqCst),
        1,
        "the recovered session must open the operation stream"
    );
}

#[tokio::test]
async fn three_nonterminal_stream_closes_are_retried_by_a_later_scan() {
    let (base_url, state) = clean_close_harness();
    let db = database().await;
    let operation = Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0011);
    seed_live(&db, operation).await;

    let first_open = state.open_attempted.notified();
    let (shutdown, task) = run_follower(follower(&base_url, &db));
    tokio::time::timeout(Duration::from_secs(1), first_open)
        .await
        .expect("the initial stream opens");
    for expected in 2..=3 {
        tokio::time::timeout(Duration::from_secs(3), state.open_attempted.notified())
            .await
            .unwrap_or_else(|_| panic!("stream open {expected} must occur inside this attempt"));
    }
    let later_scan_retried =
        tokio::time::timeout(Duration::from_secs(7), state.open_attempted.notified())
            .await
            .is_ok();

    shutdown.send_replace(true);
    task.await.expect("the follower joins");

    assert!(
        later_scan_retried,
        "a later scan must open a fourth stream for the still-live binding"
    );
    assert_eq!(state.opens.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn reconnect_after_session_expiry_uses_a_fresh_credential() {
    let clock = FakeClock::starting_now();
    let (base_url, state) = refreshed_credential_harness(clock.clone());
    let db = database().await;
    let operation = Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0012);
    seed_live(&db, operation).await;
    let sessions = sessions_with_clock(&base_url, Box::new(clock));

    let first_open = state.open_attempted.notified();
    let (shutdown, task) = run_follower(follower_with_sessions(&db, sessions));
    tokio::time::timeout(Duration::from_secs(1), first_open)
        .await
        .expect("the initial stream opens");
    tokio::time::timeout(Duration::from_secs(3), state.open_attempted.notified())
        .await
        .expect("the stream reconnects after its clean close");
    shutdown.send_replace(true);
    task.await.expect("the follower joins");

    assert_eq!(state.exchanges.load(Ordering::SeqCst), 2);
    assert_eq!(
        *state.presented.lock().expect("credentials lock"),
        vec!["Bearer session-1", "Bearer session-2"],
        "each stream open must acquire the currently valid credential"
    );
}

#[tokio::test]
async fn authentication_rejection_invalidates_only_the_rejected_cached_credential() {
    let clock = FakeClock::starting_now();
    let (base_url, state) = authentication_rejection_harness(clock.clone());
    let db = database().await;
    let operation = Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0013);
    seed_live(&db, operation).await;
    let sessions = sessions_with_clock(&base_url, Box::new(clock));

    let second_rejection = state.second_rejection_started.notified();
    let (shutdown, task) = run_follower(follower_with_sessions(&db, Arc::clone(&sessions)));
    let reached_second = tokio::time::timeout(Duration::from_secs(5), second_rejection)
        .await
        .is_ok();
    if !reached_second {
        shutdown.send_replace(true);
        state.release_second_rejection.notify_waiters();
        task.await.expect("the follower joins");
        panic!("the first rejected credential must be invalidated and re-exchanged");
    }

    state.clock.set_to(START_SECS + 7_500);
    let refreshed = sessions
        .credential(&OWNER.to_string())
        .await
        .expect("a concurrent refresh succeeds");
    assert_eq!(refreshed, "session-3");
    let accepted = state.accepted.notified();
    state.release_second_rejection.notify_waiters();
    let used_refresh = tokio::time::timeout(Duration::from_secs(5), accepted)
        .await
        .is_ok();
    shutdown.send_replace(true);
    task.await.expect("the follower joins");

    assert!(
        used_refresh,
        "rejecting session-2 must not remove the concurrently cached session-3"
    );
    assert_eq!(state.exchanges.load(Ordering::SeqCst), 3);
    assert_eq!(
        *state.presented.lock().expect("credentials lock"),
        vec!["Bearer session-1", "Bearer session-2", "Bearer session-3"]
    );
}

#[tokio::test]
async fn shutdown_cancellation_does_not_mark_a_live_follower_terminal() {
    let (base_url, state) = cancellation_harness();
    let db = database().await;
    let operation = Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0014);
    seed_live(&db, operation).await;
    let reusable = follower(&base_url, &db);

    let first_open = state.open_attempted.notified();
    let (shutdown, task) = run_follower(reusable.clone());
    tokio::time::timeout(Duration::from_secs(1), first_open)
        .await
        .expect("the first stream open is admitted");
    shutdown.send_replace(true);
    task.await.expect("the cancelled follower joins");

    let terminal: bool = sqlx::query_scalar(
        "select terminal from telegram.message_bindings where operation_id = $1",
    )
    .bind(operation)
    .fetch_one(db.pool())
    .await
    .expect("binding remains readable");
    assert!(!terminal, "shutdown cannot claim terminal completion");

    state.release_first.notify_waiters();
    let second_open = state.open_attempted.notified();
    let (restart_shutdown, restart_task) = run_follower(reusable);
    let reopened = tokio::time::timeout(Duration::from_secs(3), second_open)
        .await
        .is_ok();
    restart_shutdown.send_replace(true);
    restart_task.await.expect("the restarted follower joins");

    assert!(
        reopened,
        "cancellation cleanup must leave the live binding eligible for a later run"
    );
    assert_eq!(state.opens.load(Ordering::SeqCst), 2);
}

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
    let terminal = Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0004);
    seed_terminal(&db, terminal).await;
    db.database
        .ensure_operation_binding(BOT, terminal, CHAT + 1)
        .await
        .expect("second binding");

    let (shutdown, task) = run_follower(follower(&base_url, &db));
    wait_for_opens(&state, 3).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        state.opens.load(Ordering::SeqCst),
        3,
        "three streams for three live bindings"
    );

    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        state.opens.load(Ordering::SeqCst),
        3,
        "terminal frames cannot release in-flight ownership before durable projection"
    );
    shutdown.send_replace(true);
    task.await.expect("the follower joins");
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
    let (shutdown, task) = run_follower(follower);

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
    shutdown.send_replace(true);
    task.await.expect("the follower joins");
}
