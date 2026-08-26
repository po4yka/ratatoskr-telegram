//! The capture domain action, end to end over the real worker: intent parsing, idempotency-key
//! derivation, assertion-authenticated submission against a fake Platform, the pre-created
//! binding, the acknowledgment job, and honest failure settlement.
//!
//! Each test drives its own disposable database plus a fake Platform server. No test contacts a
//! deployed Ratatoskr deployment or Telegram.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use http::{Request, StatusCode as HttpStatus};
use http_body_util::BodyExt as _;
use ratatoskr_telegram_webhook::intake;
use ratatoskr_telegram_webhook::intake::{CaptureContext, Intake, IntakeSettings};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::Row;
use telegram_persistence::IdentityProfile;
use telegram_persistence::test_support::TestDatabase;
use tower::ServiceExt;

const BOT_ID: i64 = 700_100_200;
const OWNER_TELEGRAM_USER_ID: i64 = 900_700_601;
const SECRET: &str = "webhook-secret-0123456789abcdef";
const AUDIENCE: &str = "ratatoskr-edge-test";
const SEED: [u8; 32] = [
    7u8, 6, 5, 4, 3, 2, 1, 0, 7, 6, 5, 4, 3, 2, 1, 0, 7, 6, 5, 4, 3, 2, 1, 0, 7, 6, 5, 4, 3, 2, 1,
    0,
];
/// The operation id the fake Platform always mints; replays return the same one, per contract.
const OPERATION_ID: &str = "018f0000-0000-7000-8000-00000000cafe";

/// How the fake Platform's capture route answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureAnswer {
    /// `202` with the fixed operation id.
    Accept,
    /// `401`, the uniform credential refusal.
    RefuseAuth,
}

#[derive(Default)]
struct PlatformState {
    exchange_calls: AtomicU64,
    capture_calls: AtomicU64,
    last_idempotency_key: std::sync::Mutex<Option<String>>,
}

async fn platform_harness(answer: CaptureAnswer) -> (String, Arc<PlatformState>) {
    let state = Arc::new(PlatformState::default());
    let app_state = Arc::clone(&state);
    let app = axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            post(move |State(state): State<Arc<PlatformState>>| async move {
                state.exchange_calls.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::CREATED,
                    Json(json!({
                        "credential": "synthetic-session-credential",
                        "expires_at": "2030-01-01T00:00:00Z",
                        "user_id": OPERATION_ID,
                    })),
                )
            }),
        )
        .route(
            "/v1/captures",
            post(
                move |State(state): State<Arc<PlatformState>>,
                      headers: axum::http::HeaderMap,
                      body: axum::body::Bytes| async move {
                    state.capture_calls.fetch_add(1, Ordering::SeqCst);
                    if let Some(key) = headers.get("idempotency-key").and_then(|v| v.to_str().ok())
                    {
                        *state.last_idempotency_key.lock().expect("key lock") =
                            Some(key.to_owned());
                    }
                    let _ = serde_json::from_slice::<Value>(&body).expect("a capture body parses");
                    match answer {
                        CaptureAnswer::Accept => (
                            StatusCode::ACCEPTED,
                            Json(json!({"operation_id": OPERATION_ID, "status": "accepted"})),
                        ),
                        CaptureAnswer::RefuseAuth => (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({"error": {"code": "synthetic", "message": "refused"}})),
                        ),
                    }
                },
            ),
        )
        .with_state(app_state);
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
    (format!("http://{bound}"), state)
}

/// A dead endpoint: nothing listens, every call fails at the transport level.
fn dead_platform_url() -> String {
    "http://127.0.0.1:1".to_owned()
}

struct Fixture {
    database: TestDatabase,
    app: axum::Router,
}

impl Fixture {
    async fn create(
        base_url: &str,
        _answer: CaptureAnswer,
        platform_state: Arc<PlatformState>,
    ) -> Self {
        let database = TestDatabase::create().await.expect("disposable database");
        database
            .database
            .ensure_identity(OWNER_TELEGRAM_USER_ID, &IdentityProfile::default())
            .await
            .expect("the fixture owner identity");

        let client = platform_api::Client::new(&platform_api_url(base_url), Duration::from_secs(5))
            .expect("the platform client builds");
        let issuer = platform_api::assertion::AssertionIssuer::from_seed(&SEED, AUDIENCE)
            .expect("the issuer builds");
        let sessions = Arc::new(platform_api::session::SessionSource::new(
            client,
            issuer,
            Box::new(platform_api::session::SystemClock),
        ));

        let settings = IntakeSettings {
            secret: SecretString::new(SECRET.into()),
            max_body_bytes: 4096,
            bot_id: BOT_ID,
            queue_capacity: 32,
        };
        let (intake, receiver) = Intake::new(settings, database.database.clone());
        let context = CaptureContext::new(Arc::clone(&sessions));
        tokio::spawn(intake::run_worker(
            intake.database.clone(),
            receiver,
            Some(context),
        ));
        let _ = platform_state; // the harness state is asserted through the returned Arc
        Self {
            database,
            app: intake.router(),
        }
    }

    /// One admitted update, delivered as Telegram would.
    async fn deliver(&self, update: Value) -> HttpStatus {
        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-telegram-bot-api-secret-token", SECRET)
            .body(axum::body::Body::from(update.to_string().into_bytes()))
            .expect("the request builds");
        let response = self.app.clone().oneshot(request).await.expect("oneshot");
        let status = response.status();
        let _ = response.into_body().collect().await;
        assert_eq!(status, HttpStatus::OK, "admission must acknowledge");
        status
    }

    /// The settled state of one admitted update, polled until the deadline.
    async fn settled_state(&self, update_id: i64) -> String {
        for _ in 0..200 {
            if let Ok(row) = sqlx::query(
                "select state from telegram.updates
                 where bot_id = $1 and update_id = $2",
            )
            .bind(BOT_ID)
            .bind(update_id)
            .fetch_one(self.database.pool())
            .await
            {
                let state: String = row.get("state");
                if !matches!(state.as_str(), "accepted" | "processing") {
                    return state;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("update {update_id} never settled");
    }
}

fn platform_api_url(base: &str) -> url::Url {
    url::Url::parse(base).expect("the harness URL parses")
}

fn message_update(update_id: i64, text: &str) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": 55,
            "from": {"id": OWNER_TELEGRAM_USER_ID, "is_bot": false, "first_name": "Synthetic"},
            "date": 1_760_000_000_i64,
            "chat": {"id": OWNER_TELEGRAM_USER_ID, "type": "private",
                     "first_name": "Synthetic"},
            "text": text,
        },
    })
}

async fn outbound_job_count(fixture: &Fixture) -> i64 {
    sqlx::query_scalar("select count(*) from telegram.outbound_jobs")
        .fetch_one(fixture.database.pool())
        .await
        .expect("job count")
}

async fn binding_count(fixture: &Fixture, operation_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar("select count(*) from telegram.message_bindings where operation_id = $1")
        .bind(operation_id)
        .fetch_one(fixture.database.pool())
        .await
        .expect("binding count")
}

/// An authorized bare URL submits one capture under a derived idempotency key and enqueues
/// exactly one acknowledgment job referencing the returned operation.
#[tokio::test]
async fn authorized_url_message_submits_capture_and_enqueues_ack() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;
    fixture
        .deliver(message_update(9_001, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_001).await, "processed");

    assert_eq!(state.exchange_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 1);

    let operation: uuid::Uuid = OPERATION_ID.parse().expect("synthetic uuid");
    assert_eq!(binding_count(&fixture, operation).await, 1);
    assert_eq!(outbound_job_count(&fixture).await, 1);

    let job: (String, Option<uuid::Uuid>, Option<String>, Option<String>) = sqlx::query_as(
        "select kind, operation_id, correlation_id,
                payload->>'parse_mode'
         from telegram.outbound_jobs",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("the ack job");
    assert_eq!(job.0, "send_message");
    assert_eq!(job.1, Some(operation));
    let expected_correlation = format!("operation:{OPERATION_ID}");
    assert_eq!(job.2.as_deref(), Some(expected_correlation.as_str()));
    assert_eq!(job.3.as_deref(), Some("HTML"), "the ack carries HTML");

    let intents: i64 = sqlx::query_scalar("select count(*) from telegram.interaction_intents")
        .fetch_one(fixture.database.pool())
        .await
        .expect("intent count");
    assert_eq!(intents, 1, "one deep-link intent backs the future button");
}

/// Resending the same link reuses the operation and does not enqueue a second acknowledgment.
#[tokio::test]
async fn resending_the_same_url_reuses_the_operation_without_a_second_ack() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;
    fixture
        .deliver(message_update(9_101, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_101).await, "processed");
    fixture
        .deliver(message_update(9_102, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_102).await, "processed");

    assert_eq!(
        state.capture_calls.load(Ordering::SeqCst),
        2,
        "both deliveries submitted; Platform replays the original operation"
    );
    let operation: uuid::Uuid = OPERATION_ID.parse().expect("synthetic uuid");
    assert_eq!(binding_count(&fixture, operation).await, 1);
    assert_eq!(
        outbound_job_count(&fixture).await,
        1,
        "one tracked message, not one per resend"
    );

    // The two submissions carried the same derived key - that is what made them converge.
    let key = state
        .last_idempotency_key
        .lock()
        .expect("key lock")
        .clone()
        .expect("a key was sent");
    assert_eq!(key.len(), 64, "the key is lowercase hex sha256");
}

/// `/summarize <url>` behaves exactly like the bare form.
#[tokio::test]
async fn summarize_command_parses_like_a_bare_url() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;
    fixture
        .deliver(message_update(
            9_201,
            "/summarize https://example.test/article",
        ))
        .await;
    assert_eq!(fixture.settled_state(9_201).await, "processed");
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 1);
    let body_text: String =
        sqlx::query_scalar("select payload->>'text' from telegram.outbound_jobs limit 1")
            .fetch_one(fixture.database.pool())
            .await
            .expect("ack body");
    assert!(
        body_text.contains("<a href=\"https://example.test/article\">"),
        "the ack links the captured address: {body_text}"
    );
}

/// Text without an intent settles unsupported and never reaches Platform.
#[tokio::test]
async fn unsupported_text_settles_unsupported_without_platform_calls() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;
    fixture.deliver(message_update(9_301, "hello world")).await;
    assert_eq!(fixture.settled_state(9_301).await, "unsupported");
    fixture.deliver(message_update(9_302, "/summarize")).await;
    assert_eq!(fixture.settled_state(9_302).await, "unsupported");
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 0);
    assert_eq!(outbound_job_count(&fixture).await, 0);
}

/// An unreachable Platform fails the update boundedly, counting the class, sending nothing.
#[tokio::test]
async fn platform_outage_fails_boundedly_without_ack() {
    let dead = dead_platform_url();
    let fixture = Fixture::create(&dead, CaptureAnswer::Accept, Arc::default()).await;
    fixture
        .deliver(message_update(9_401, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_401).await, "failed");
    assert_eq!(outbound_job_count(&fixture).await, 0);
}

/// A permanent refusal settles immediately: exactly one attempt reaches Platform.
#[tokio::test]
async fn permanent_refusal_settles_immediately() {
    let (base_url, state) = platform_harness(CaptureAnswer::RefuseAuth).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::RefuseAuth, Arc::clone(&state)).await;
    fixture
        .deliver(message_update(9_501, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_501).await, "failed");
    assert_eq!(
        state.capture_calls.load(Ordering::SeqCst),
        1,
        "a permanent class must not retry"
    );
    assert_eq!(outbound_job_count(&fixture).await, 0);
}
