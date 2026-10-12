//! Capture behavior end to end over the real worker and synthetic local dependencies.
//! No test contacts a deployed Ratatoskr service or Telegram.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

mod capture_attachments;
mod capture_failures;
mod capture_identity;
mod capture_recovery;
mod capture_submission;

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
use sha2::{Digest as _, Sha256};
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
/// The static reply bodies of XR-021 CONTRACTS S11, spelled out so a wording change is a visible
/// contract change in this file.
const UNSUPPORTED_BODY: &str = "<b>Unsupported attachment</b>\nSend a PDF document with a text layer; images, video, voice, and audio are not supported yet.";
const UNAVAILABLE_BODY: &str = "<b>Ratatoskr is temporarily unavailable</b>\nYour item was not saved. Please send it again in a few minutes.";
const REFUSED_BODY: &str = "<b>Could not save this item</b>\nRatatoskr did not accept it. If this keeps happening, contact the operator.";
const UNREADABLE_BODY: &str = "<b>Could not read the attachment</b>\nPlease send it again.";
/// The operation id the fake Platform always mints; replays return the same one, per contract.
const OPERATION_ID: &str = "018f0000-0000-7000-8000-00000000cafe";
/// How the fake Platform's capture route answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureAnswer {
    /// `202` with the fixed operation id.
    Accept,
    /// `401`, the uniform credential refusal.
    RefuseAuth,
    /// `400`, a permanent refusal of the request itself.
    RefuseBadRequest,
    /// `503`, a transient outage on every attempt.
    Unavailable,
    /// The real idempotency ledger: the same key with the same body replays its operation, the
    /// same key with another body is `409`, and every new key mints a new operation.
    Ledger,
    /// `409` on every attempt.
    ConflictAlways,
    /// `409` on the first attempt, then `202`.
    ConflictOnce,
    /// `401` for the first minted session credential, `202` for every later one.
    RejectFirstSession,
}
#[derive(Default)]
struct PlatformState {
    exchange_calls: AtomicU64,
    capture_calls: AtomicU64,
    get_file_calls: AtomicU64,
    download_calls: AtomicU64,
    last_get_file_id: std::sync::Mutex<Option<String>>,
    last_idempotency_key: std::sync::Mutex<Option<String>>,
    idempotency_keys: std::sync::Mutex<Vec<String>>,
    /// The most recent capture request body, for wire-shape assertions.
    last_capture_body: std::sync::Mutex<Option<Value>>,
    /// The path of every capture request, in arrival order.
    capture_paths: std::sync::Mutex<Vec<String>>,
    /// When set, the file download route answers `500` instead of the attachment bytes.
    download_fails: std::sync::atomic::AtomicBool,
    attachment_bytes: std::sync::Mutex<Vec<u8>>,
    /// The `Ledger` answer's memory: every key with the exact body first sent under it.
    ledger: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
    /// The bearer credential of every capture request, in arrival order.
    bearers: std::sync::Mutex<Vec<String>>,
}

async fn platform_harness(answer: CaptureAnswer) -> (String, Arc<PlatformState>) {
    let state = Arc::new(PlatformState {
        attachment_bytes: std::sync::Mutex::new(b"synthetic attachment".to_vec()),
        ..PlatformState::default()
    });
    let app = platform_router(answer, Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let bound = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).into_future().await;
    });
    (format!("http://{bound}"), state)
}

fn platform_router(answer: CaptureAnswer, state: Arc<PlatformState>) -> axum::Router {
    axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            post(move |State(state): State<Arc<PlatformState>>| async move {
                // Every exchange mints a distinguishable credential, so a test can tell a
                // dropped session from a reused one.
                let minted = state.exchange_calls.fetch_add(1, Ordering::SeqCst) + 1;
                (
                    StatusCode::CREATED,
                    Json(json!({
                        "credential": format!("synthetic-session-credential-{minted}"),
                        "expires_at": "2030-01-01T00:00:00Z",
                        "user_id": OPERATION_ID,
                    })),
                )
            }),
        )
        .route("/v1/captures", capture_route(answer, "/v1/captures"))
        .route(
            "/v1/captures/blobs",
            capture_route(answer, "/v1/captures/blobs"),
        )
        .route(
            "/botsynthetic-bot-token/GetFile",
            post(
                move |State(state): State<Arc<PlatformState>>, Json(body): Json<Value>| async move {
                    state.get_file_calls.fetch_add(1, Ordering::SeqCst);
                    *state.last_get_file_id.lock().expect("file id lock") = body
                        .get("file_id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    let size = state
                        .attachment_bytes
                        .lock()
                        .expect("attachment lock")
                        .len();
                    (
                        StatusCode::OK,
                        Json(json!({
                            "ok": true,
                            "result": {
                                "file_id": "synthetic-file",
                                "file_unique_id": "synthetic-unique",
                                "file_size": size,
                                "file_path": "attachments/synthetic"
                            }
                        })),
                    )
                },
            ),
        )
        .route(
            "/file/botsynthetic-bot-token/attachments/synthetic",
            axum::routing::get(move |State(state): State<Arc<PlatformState>>| async move {
                state.download_calls.fetch_add(1, Ordering::SeqCst);
                if state.download_fails.load(Ordering::SeqCst) {
                    return (StatusCode::INTERNAL_SERVER_ERROR, Vec::new());
                }
                (
                    StatusCode::OK,
                    state
                        .attachment_bytes
                        .lock()
                        .expect("attachment lock")
                        .clone(),
                )
            }),
        )
        .with_state(state)
}

/// One capture route of the fake Platform: records the request, then answers as configured.
fn capture_route(
    answer: CaptureAnswer,
    path: &'static str,
) -> axum::routing::MethodRouter<Arc<PlatformState>> {
    post(
        move |State(state): State<Arc<PlatformState>>,
              headers: axum::http::HeaderMap,
              body: axum::body::Bytes| async move {
            let call = state.capture_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let bearer = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            state
                .bearers
                .lock()
                .expect("bearer lock")
                .push(bearer.clone());
            state
                .capture_paths
                .lock()
                .expect("path lock")
                .push(path.to_owned());
            if let Some(key) = headers.get("idempotency-key").and_then(|v| v.to_str().ok()) {
                let key = key.to_owned();
                *state.last_idempotency_key.lock().expect("key lock") = Some(key.clone());
                state
                    .idempotency_keys
                    .lock()
                    .expect("key history lock")
                    .push(key);
            }
            let parsed = serde_json::from_slice::<Value>(&body).expect("a capture body parses");
            *state.last_capture_body.lock().expect("body lock") = Some(parsed);
            let refusal = |status: StatusCode| {
                (
                    status,
                    Json(json!({"error": {"code": "synthetic", "message": "refused"}})),
                )
            };
            let accepted = |operation_id: &str| {
                (
                    StatusCode::ACCEPTED,
                    Json(json!({"operation_id": operation_id, "status": "accepted"})),
                )
            };
            match answer {
                CaptureAnswer::Ledger => {
                    match capture_submission::ledger_verdict(&state, &headers, &body) {
                        Some(operation_id) => accepted(&operation_id),
                        None => refusal(StatusCode::CONFLICT),
                    }
                }
                CaptureAnswer::ConflictAlways => refusal(StatusCode::CONFLICT),
                CaptureAnswer::ConflictOnce if call == 1 => refusal(StatusCode::CONFLICT),
                CaptureAnswer::RejectFirstSession if bearer.ends_with("-1") => {
                    refusal(StatusCode::UNAUTHORIZED)
                }
                CaptureAnswer::Accept
                | CaptureAnswer::ConflictOnce
                | CaptureAnswer::RejectFirstSession => accepted(OPERATION_ID),
                CaptureAnswer::RefuseAuth => refusal(StatusCode::UNAUTHORIZED),
                CaptureAnswer::RefuseBadRequest => refusal(StatusCode::BAD_REQUEST),
                CaptureAnswer::Unavailable => refusal(StatusCode::SERVICE_UNAVAILABLE),
            }
        },
    )
}

/// A dead endpoint: nothing listens, every call fails at the transport level.
fn dead_platform_url() -> String {
    "http://127.0.0.1:1".to_owned()
}

struct Fixture {
    database: TestDatabase,
    app: axum::Router,
    blob_root: tempfile::TempDir,
}

impl Fixture {
    async fn create(
        base_url: &str,
        _answer: CaptureAnswer,
        platform_state: Arc<PlatformState>,
    ) -> Self {
        let (fixture, receiver, context) = Self::prepare(base_url).await;
        tokio::spawn(intake::run_worker(
            fixture.database.database.clone(),
            receiver,
            Some(context),
        ));
        let _ = platform_state; // the harness state is asserted through the returned Arc
        fixture
    }

    /// Build an admitted-update fixture without starting its worker.
    async fn prepare(
        base_url: &str,
    ) -> (
        Self,
        tokio::sync::mpsc::Receiver<intake::QueuedUpdate>,
        CaptureContext,
    ) {
        let database = TestDatabase::create().await.expect("disposable database");
        database
            .database
            .ensure_identity(OWNER_TELEGRAM_USER_ID, &IdentityProfile::default())
            .await
            .expect("the fixture owner identity");

        let platform = telegram_core::PlatformConfig {
            base_url: platform_api_url(base_url),
            timeout_seconds: 5,
            audience: AUDIENCE.to_owned(),
            assertion_signing_key: SecretString::new(hex_seed().into()),
            ..telegram_core::PlatformConfig::default()
        };
        let bot_api = bot_api::Client::new(
            &SecretString::new("synthetic-bot-token".into()),
            &platform_api_url(base_url),
            Duration::from_secs(5),
        )
        .expect("the synthetic Bot API client builds");
        let blob_root = tempfile::tempdir().expect("blob root");
        // The production factory, so the identity-binding hook under test is the one that ships.
        let context = intake::build_capture_context(
            &platform,
            bot_api,
            &database.database,
            1024,
            blob_root.path(),
        )
        .expect("the capture context builds");

        let settings = IntakeSettings {
            secret: SecretString::new(SECRET.into()),
            max_body_bytes: 4096,
            bot_id: BOT_ID,
            queue_capacity: 32,
        };
        let (intake, receiver) = Intake::new(settings, database.database.clone());
        (
            Self {
                database,
                app: intake.router(),
                blob_root,
            },
            receiver,
            context,
        )
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

    /// A successful store creates a digest-named content tree. A failed stream must not.
    fn has_published_blob(&self) -> bool {
        self.blob_root.path().join("sha256").exists()
    }
}

/// The test signing seed as the 64 hex characters the configuration carries.
fn hex_seed() -> String {
    use std::fmt::Write as _;
    SEED.iter().fold(String::new(), |mut hex, byte| {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
        hex
    })
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

/// A private message forwarding a channel post; `text` is the forwarded post's text.
fn forwarded_update(update_id: i64, text: &str) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": 56,
            "from": {"id": OWNER_TELEGRAM_USER_ID, "is_bot": false, "first_name": "Synthetic"},
            "date": 1_760_000_100_i64,
            "chat": {"id": OWNER_TELEGRAM_USER_ID, "type": "private",
                     "first_name": "Synthetic"},
            "forward_origin": {
                "type": "channel",
                "chat": {"id": -100_200_300, "type": "channel", "title": "Synthetic Channel"},
                "message_id": 77,
                "date": 1_700_000_000_i64,
            },
            "text": text,
        },
    })
}

/// A document update with only the Bot API metadata the intake must trust before download.
fn document_update(update_id: i64, file_id: &str, file_size: u64, mime_type: &str) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": 57,
            "from": {"id": OWNER_TELEGRAM_USER_ID, "is_bot": false, "first_name": "Synthetic"},
            "date": 1_760_000_200_i64,
            "chat": {"id": OWNER_TELEGRAM_USER_ID, "type": "private",
                     "first_name": "Synthetic"},
            "document": {
                "file_id": file_id,
                "file_unique_id": "document-unique-id",
                "file_size": file_size,
                "file_name": "sample.pdf",
                "thumbnail": null,
                "mime_type": mime_type
            }
        }
    })
}

fn pdf_document_update(update_id: i64, file_id: &str, file_size: u64) -> Value {
    document_update(update_id, file_id, file_size, "application/pdf")
}

fn photo_update(update_id: i64) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": 58,
            "from": {"id": OWNER_TELEGRAM_USER_ID, "is_bot": false, "first_name": "Synthetic"},
            "date": 1_760_000_300_i64,
            "chat": {"id": OWNER_TELEGRAM_USER_ID, "type": "private", "first_name": "Synthetic"},
            "photo": [
                {"file_id": "photo-small", "file_unique_id": "photo-small-unique", "width": 16, "height": 16, "file_size": 128},
                {"file_id": "photo-largest-eligible", "file_unique_id": "photo-largest-eligible-unique", "width": 64, "height": 64, "file_size": 768},
                {"file_id": "photo-over-limit", "file_unique_id": "photo-over-limit-unique", "width": 128, "height": 128, "file_size": 1025}
            ]
        }
    })
}

fn unsupported_media_update(update_id: i64, field: &str) -> Value {
    let media = match field {
        "voice" => json!({
            "file_id": "voice-file", "file_unique_id": "voice-unique", "duration": 1,
            "mime_type": "audio/ogg", "file_size": 12
        }),
        "video" => json!({
            "file_id": "video-file", "file_unique_id": "video-unique", "duration": 1,
            "width": 16, "height": 16, "mime_type": "video/mp4", "file_size": 12
        }),
        _ => unreachable!("only voice and video synthetic updates are supported"),
    };
    json!({
        "update_id": update_id,
        "message": {
            "message_id": 59,
            "from": {"id": OWNER_TELEGRAM_USER_ID, "is_bot": false, "first_name": "Synthetic"},
            "date": 1_760_000_400_i64,
            "chat": {"id": OWNER_TELEGRAM_USER_ID, "type": "private", "first_name": "Synthetic"},
            field: media,
        }
    })
}

async fn outbound_job_count(fixture: &Fixture) -> i64 {
    sqlx::query_scalar("select count(*) from telegram.outbound_jobs")
        .fetch_one(fixture.database.pool())
        .await
        .expect("job count")
}

/// The text of every queued outbound job, oldest first.
async fn outbound_bodies(fixture: &Fixture) -> Vec<String> {
    sqlx::query_scalar("select payload->>'text' from telegram.outbound_jobs order by id")
        .fetch_all(fixture.database.pool())
        .await
        .expect("outbound bodies")
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

    let intents: i64 = sqlx::query_scalar(
        "select count(*) from telegram.interaction_tokens where surface = 'deep_link'",
    )
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

/// A forwarded channel post carrying a link submits the ordinary URL capture with the forward
/// origin preserved - in the submission body and on the persisted intent record.
#[tokio::test]
async fn forwarded_message_with_link_submits_capture_with_origin() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;
    fixture
        .deliver(forwarded_update(9_601, "https://example.test/story"))
        .await;
    assert_eq!(fixture.settled_state(9_601).await, "processed");

    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 1);
    let body = state
        .last_capture_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("a capture body was posted");
    assert_eq!(body["url"], "https://example.test/story");
    assert_eq!(
        body["origin"]["forward"],
        json!({"kind": "channel", "chat_id": -100_200_300, "message_id": 77,
               "sent_at_secs": 1_700_000_000}),
        "the submission carries the minimized origin: {body}"
    );

    // The intent record persists the same provenance beside the address.
    let origin_kind: Option<String> = sqlx::query_scalar(
        "select payload->'metadata'->'forward'->>'kind' from telegram.interaction_tokens
         where payload->>'source_url' = 'https://example.test/story'",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("the forwarded intent row");
    assert_eq!(origin_kind.as_deref(), Some("channel"));

    let operation: uuid::Uuid = OPERATION_ID.parse().expect("synthetic uuid");
    assert_eq!(binding_count(&fixture, operation).await, 1);
    assert_eq!(outbound_job_count(&fixture).await, 1);
}

/// A forwarded PDF: the `forward` member is the only provenance that may reach Platform inside
/// `origin` (XR-021 CONTRACTS R2-17). The attachment facts are the `blob` member's job and stay
/// Telegram-local everywhere else; a PDF that is not a forward sends no `origin` at all.
#[tokio::test]
async fn a_forwarded_pdf_sends_only_the_forward_member_as_origin() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    let mut forwarded = pdf_document_update(9_901, "pdf-forwarded", 128);
    forwarded["message"]["forward_origin"] = json!({
        "type": "channel",
        "chat": {"id": -100_200_300, "type": "channel", "title": "Synthetic Channel"},
        "message_id": 77,
        "date": 1_700_000_000_i64,
    });
    fixture.deliver(forwarded).await;
    assert_eq!(fixture.settled_state(9_901).await, "processed");
    assert_eq!(
        *state.capture_paths.lock().expect("path lock"),
        ["/v1/captures/blobs"]
    );

    let attachment = state
        .attachment_bytes
        .lock()
        .expect("attachment lock")
        .clone();
    let digest = format!("{:x}", Sha256::digest(&attachment));
    let body = state
        .last_capture_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("a capture body was posted");
    assert_eq!(
        body["blob"],
        json!({
            "owner_service": "ratatoskr-telegram",
            "digest": {"algorithm": "sha256", "hex": digest},
            "media_type": "application/pdf",
            "length_bytes": attachment.len(),
        }),
        "the blob member is unchanged: {body}"
    );
    assert_eq!(
        body["origin"],
        json!({"forward": {"kind": "channel", "chat_id": -100_200_300, "message_id": 77,
                           "sent_at_secs": 1_700_000_000}}),
        "origin carries the forward provenance and nothing else: {body}"
    );
    let origin = body["origin"].to_string();
    for forbidden in [
        "blob",
        "owner_service",
        "digest",
        "media_type",
        "length_bytes",
        "algorithm",
    ] {
        assert!(
            !origin.contains(forbidden),
            "origin leaks attachment fact {forbidden}: {origin}"
        );
    }

    // The Telegram-local intent row still stores the full metadata.
    let metadata: Value = sqlx::query_scalar(
        "select payload->'metadata' from telegram.interaction_tokens where operation_id = $1",
    )
    .bind(OPERATION_ID.parse::<uuid::Uuid>().expect("synthetic uuid"))
    .fetch_one(fixture.database.pool())
    .await
    .expect("the attachment intent row");
    assert_eq!(metadata["forward"]["kind"], "channel");
    assert_eq!(metadata["blob"]["owner_service"], "ratatoskr-telegram");
    assert_eq!(metadata["blob"]["digest_hex"], digest);

    // A PDF that is not a forward sends no `origin` member at all.
    fixture
        .deliver(pdf_document_update(9_902, "pdf-plain", 128))
        .await;
    assert_eq!(fixture.settled_state(9_902).await, "processed");
    let plain = state
        .last_capture_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("a second capture body was posted");
    assert!(
        plain.get("origin").is_none(),
        "a forward-less PDF sends no origin: {plain}"
    );
    assert!(plain.get("blob").is_some());
}

/// The first link in a forward wins; a forward without any link settles unsupported.
#[tokio::test]
async fn first_forwarded_link_wins_and_linkless_forwards_stay_unsupported() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    fixture
        .deliver(forwarded_update(
            9_701,
            "read https://a.test/first and also https://b.test/second",
        ))
        .await;
    assert_eq!(fixture.settled_state(9_701).await, "processed");
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 1);
    let body = state
        .last_capture_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("a capture body was posted");
    assert_eq!(
        body["url"], "https://a.test/first",
        "exactly one capture, for the first link"
    );

    fixture
        .deliver(forwarded_update(9_702, "just a note, nothing to capture"))
        .await;
    assert_eq!(fixture.settled_state(9_702).await, "unsupported");
    assert_eq!(
        state.capture_calls.load(Ordering::SeqCst),
        1,
        "the linkless forward never reached Platform"
    );
    assert_eq!(outbound_job_count(&fixture).await, 1);
}
