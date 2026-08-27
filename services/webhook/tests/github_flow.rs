//! GitHub repository preview, confirmation gating, and truthful partial-result acceptance.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{Json, http::StatusCode, routing::post};
use http::Request;
use http_body_util::BodyExt as _;
use ratatoskr_telegram_webhook::intake::{self, CaptureContext, Intake, IntakeSettings};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::Row as _;
use telegram_persistence::{IdentityProfile, test_support::TestDatabase};
use tower::ServiceExt as _;

const BOT: i64 = 700_100_200;
const OWNER: i64 = 900_700_601;
const SECRET: &str = "webhook-secret-0123456789abcdef";
const SEED: [u8; 32] = [7; 32];

#[derive(Default)]
struct Calls {
    previews: AtomicU64,
    actions: AtomicU64,
    callback_answers: AtomicU64,
    action_keys: Mutex<Vec<String>>,
}

async fn harness(fail_actions: bool) -> (url::Url, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let app = axum::Router::new()
        .route("/v1/sessions/telegram", post(|| async {
            (StatusCode::CREATED, Json(json!({"credential":"session", "expires_at":"2030-01-01T00:00:00Z", "user_id":"018f0000-0000-7000-8000-00000000cafe"})))
        }))
        .route("/v1/gh/repositories/preview", post({
            let calls = Arc::clone(&calls);
            move || { let calls = Arc::clone(&calls); async move {
                calls.previews.fetch_add(1, Ordering::SeqCst);
                Json(json!({
                    "target":{"github_repository_numeric_id":42,"repository_full_name":"owner/repository","canonical_url":"https://github.com/owner/repository"},
                    "description":"A <tool>","stargazer_count":42,"primary_language":"Rust",
                    "account_ref":"github-account:018f0000-0000-7000-8000-000000000604",
                    "available_actions":["metadata","track","star"]
                }))
            }}
        }))
        .route("/v1/gh/repositories/actions", post({
            let calls = Arc::clone(&calls);
            move |body: axum::body::Bytes| { let calls = Arc::clone(&calls); async move {
                let request: Value = serde_json::from_slice(&body).expect("action request");
                assert_eq!(request.get("mode").and_then(Value::as_str), Some("star"));
                assert!(request.get("confirmation_evidence_ref").and_then(Value::as_str).is_some());
                assert!(request.get("idempotency_key").and_then(Value::as_str).is_some());
                calls.action_keys.lock().expect("keys").push(request.get("idempotency_key").and_then(Value::as_str).unwrap_or_default().to_owned());
                calls.actions.fetch_add(1, Ordering::SeqCst);
                if fail_actions {
                    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":{"code":"temporary","message":"unavailable"}})))
                } else {
                    (StatusCode::OK, Json(json!({
                        "aggregate":"partial","metadata":{"status":"succeeded"},
                        "provider_star":{"status":"succeeded"},
                        "desired_backup":{"status":"failed","reason":"policy_publication_failed"}
                    })))
                }
            }}
        }))
        .route("/botsynthetic-bot-token/AnswerCallbackQuery", post({
            let calls = Arc::clone(&calls);
            move || { let calls = Arc::clone(&calls); async move {
                calls.callback_answers.fetch_add(1, Ordering::SeqCst);
                Json(json!({"ok":true,"result":true}))
            }}
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
    (
        url::Url::parse(&format!("http://{address}")).expect("url"),
        calls,
    )
}

struct Fixture {
    database: TestDatabase,
    app: axum::Router,
}

impl Fixture {
    async fn create(base: &url::Url) -> Self {
        let database = TestDatabase::create().await.expect("database");
        database
            .database
            .ensure_identity(OWNER, &IdentityProfile::default())
            .await
            .expect("owner");
        let platform = platform_api::Client::new(base, Duration::from_secs(2)).expect("platform");
        let issuer =
            platform_api::assertion::AssertionIssuer::from_seed(&SEED, "ratatoskr-edge-test")
                .expect("issuer");
        let sessions = Arc::new(platform_api::session::SessionSource::new(
            platform,
            issuer,
            Box::new(platform_api::session::SystemClock),
        ));
        let bot = bot_api::Client::new(
            &SecretString::new("synthetic-bot-token".into()),
            base,
            Duration::from_secs(2),
        )
        .expect("bot");
        let root = tempfile::tempdir().expect("root").keep();
        let blobs = ratatoskr_telegram_blob_store::BlobStore::open(&root).expect("blobs");
        let (intake, receiver) = Intake::new(
            IntakeSettings {
                secret: SecretString::new(SECRET.into()),
                max_body_bytes: 8192,
                bot_id: BOT,
                queue_capacity: 32,
            },
            database.database.clone(),
        );
        tokio::spawn(intake::run_worker(
            intake.database.clone(),
            receiver,
            Some(CaptureContext::new(sessions, bot, blobs, 1024)),
        ));
        Self {
            database,
            app: intake.router(),
        }
    }

    async fn deliver(&self, update: Value) {
        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-telegram-bot-api-secret-token", SECRET)
            .body(axum::body::Body::from(update.to_string()))
            .expect("request");
        let response = self.app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.into_body().collect().await;
    }

    async fn wait_settled(&self, update_id: i64) {
        for _ in 0..200 {
            if let Ok(state) = sqlx::query_scalar::<_, String>(
                "select state from telegram.updates where bot_id=$1 and update_id=$2",
            )
            .bind(BOT)
            .bind(update_id)
            .fetch_one(self.database.pool())
            .await
                && !matches!(state.as_str(), "accepted" | "processing")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("update did not settle");
    }
}

fn message(update_id: i64) -> Value {
    json!({"update_id":update_id,"message":{"message_id":55,"from":{"id":OWNER,"is_bot":false,"first_name":"Owner"},"date":1_760_000_000,"chat":{"id":OWNER,"type":"private","first_name":"Owner"},"text":"https://github.com/owner/repository"}})
}

fn callback(update_id: i64, message_id: i64, data: &str, actor: i64) -> Value {
    json!({"update_id":update_id,"callback_query":{"id":format!("callback-{update_id}"),"from":{"id":actor,"is_bot":false,"first_name":"Owner"},"message":{"message_id":message_id,"from":{"id":BOT,"is_bot":true,"first_name":"Bot"},"date":1_760_000_001,"chat":{"id":OWNER,"type":"private","first_name":"Owner"},"text":"synthetic"},"chat_instance":"private","data":data}})
}

async fn token(database: &TestDatabase, action: &str) -> String {
    sqlx::query("select token from telegram.callback_tokens where action=$1 order by expires_at desc limit 1")
        .bind(action).fetch_one(database.pool()).await.expect("token").get("token")
}

#[tokio::test]
async fn repository_preview_confirmation_gate_and_partial_result_are_truthful() {
    let (base, calls) = harness(false).await;
    let fixture = Fixture::create(&base).await;
    fixture.deliver(message(10_001)).await;
    fixture.wait_settled(10_001).await;
    assert_eq!(calls.previews.load(Ordering::SeqCst), 1);
    assert_eq!(
        calls.actions.load(Ordering::SeqCst),
        0,
        "preview cannot write"
    );
    let preview_body: String = sqlx::query_scalar(
        "select payload->>'text' from telegram.outbound_jobs order by id limit 1",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("preview job");
    assert_eq!(
        preview_body,
        "<b>GitHub repository</b>\nowner/repository\nA &lt;tool&gt;\nStars: 42\nLanguage: Rust"
    );

    let flow_id: uuid::Uuid = sqlx::query_scalar("select id from telegram.callback_flows")
        .fetch_one(fixture.database.pool())
        .await
        .expect("flow");
    fixture
        .database
        .database
        .stamp_callback_message(flow_id, BOT, OWNER, 100, 1_800_000_000)
        .await
        .expect("stamp");
    let select_star = token(&fixture.database, "select_star").await;
    fixture
        .deliver(callback(10_002, 100, &select_star, OWNER))
        .await;
    fixture.wait_settled(10_002).await;
    assert_eq!(
        calls.actions.load(Ordering::SeqCst),
        0,
        "selection cannot write"
    );

    fixture
        .database
        .database
        .stamp_callback_message(flow_id, BOT, OWNER, 101, 1_800_000_001)
        .await
        .expect("stamp confirmation");
    let confirm = token(&fixture.database, "confirm").await;
    fixture
        .deliver(callback(10_003, 101, &confirm, OWNER))
        .await;
    fixture.wait_settled(10_003).await;
    assert_eq!(
        calls.actions.load(Ordering::SeqCst),
        1,
        "confirmed token writes once"
    );
    assert_eq!(calls.callback_answers.load(Ordering::SeqCst), 2);
    let result: String = sqlx::query_scalar(
        "select payload->>'text' from telegram.outbound_jobs order by id desc limit 1",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("result");
    assert!(result.contains("partially completed"), "{result}");
    assert!(result.contains("GitHub star: succeeded"), "{result}");
    assert!(
        result.contains("Desired backup: failed: desired-policy publication"),
        "{result}"
    );
    assert!(!result.contains("backup completed"));
}

#[tokio::test]
async fn uncertain_action_retries_only_the_same_identity_and_never_claims_success() {
    let (base, calls) = harness(true).await;
    let fixture = Fixture::create(&base).await;
    fixture.deliver(message(11_001)).await;
    fixture.wait_settled(11_001).await;
    let flow_id: uuid::Uuid = sqlx::query_scalar("select id from telegram.callback_flows")
        .fetch_one(fixture.database.pool())
        .await
        .expect("flow");
    fixture
        .database
        .database
        .stamp_callback_message(flow_id, BOT, OWNER, 100, 1_800_000_000)
        .await
        .expect("stamp");
    let select_star = token(&fixture.database, "select_star").await;
    fixture
        .deliver(callback(11_002, 100, &select_star, OWNER))
        .await;
    fixture.wait_settled(11_002).await;
    fixture
        .database
        .database
        .stamp_callback_message(flow_id, BOT, OWNER, 101, 1_800_000_001)
        .await
        .expect("stamp confirmation");
    let confirm = token(&fixture.database, "confirm").await;
    fixture
        .deliver(callback(11_003, 101, &confirm, OWNER))
        .await;
    fixture.wait_settled(11_003).await;

    assert_eq!(calls.actions.load(Ordering::SeqCst), 2, "bounded retry");
    let keys = calls.action_keys.lock().expect("keys").clone();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys.first(), keys.last(), "retries must reuse one identity");
    let result: String = sqlx::query_scalar(
        "select payload->>'text' from telegram.outbound_jobs order by id desc limit 1",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("unknown result");
    assert!(result.contains("outcome unknown"), "{result}");
    assert!(!result.contains("succeeded"), "{result}");
}
