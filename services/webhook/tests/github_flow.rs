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

#[derive(Clone, Copy)]
enum ActionAnswer {
    Success,
    Unavailable,
    Refused,
}

async fn harness(action_answer: ActionAnswer) -> (url::Url, Arc<Calls>) {
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
                match action_answer {
                    ActionAnswer::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":{"code":"temporary","message":"unavailable"}}))),
                    ActionAnswer::Refused => (StatusCode::FORBIDDEN, Json(json!({"error":{"code":"refused","message":"refused"}}))),
                    ActionAnswer::Success => (StatusCode::OK, Json(json!({
                        "aggregate":"partial","metadata":{"status":"succeeded"},
                        "provider_star":{"status":"succeeded"},
                        "desired_backup":{"status":"failed","reason":"policy_publication_failed"}
                    }))),
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
        for _ in 0..600 {
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
    sqlx::query("select token from telegram.interaction_tokens where action=$1 order by expires_at desc limit 1")
        .bind(action).fetch_one(database.pool()).await.expect("token").get("token")
}

async fn prepare_confirmation(
    fixture: &Fixture,
    preview_update_id: i64,
    selection_update_id: i64,
) -> (uuid::Uuid, String) {
    fixture.deliver(message(preview_update_id)).await;
    fixture.wait_settled(preview_update_id).await;
    let dialogue_id: uuid::Uuid = sqlx::query_scalar("select id from telegram.dialog_states")
        .fetch_one(fixture.database.pool())
        .await
        .expect("dialogue");
    fixture
        .database
        .database
        .stamp_callback_message(dialogue_id, BOT, OWNER, 100, 1_800_000_000)
        .await
        .expect("stamp selection");
    let select_star = token(&fixture.database, "select_star").await;
    fixture
        .deliver(callback(selection_update_id, 100, &select_star, OWNER))
        .await;
    fixture.wait_settled(selection_update_id).await;
    fixture
        .database
        .database
        .stamp_callback_message(dialogue_id, BOT, OWNER, 101, 1_800_000_001)
        .await
        .expect("stamp confirmation");
    let confirm = token(&fixture.database, "confirm").await;
    (dialogue_id, confirm)
}

async fn reject_repository_result_jobs(database: &TestDatabase) {
    sqlx::query("create sequence telegram.atomic_result_fault_sequence start with 1")
        .execute(database.pool())
        .await
        .expect("atomic result fault sequence");
    sqlx::query(
        "create function telegram.reject_repository_result_job() returns trigger
         language plpgsql as $$
         begin
             if new.kind = 'send_message'
                and new.payload->>'text' like '<b>Repository action%' then
                 perform nextval('telegram.atomic_result_fault_sequence');
                 raise exception 'injected repository result storage failure';
             end if;
             return new;
         end
         $$",
    )
    .execute(database.pool())
    .await
    .expect("result failure function");
    sqlx::query(
        "create trigger reject_repository_result_job
         before insert on telegram.outbound_jobs
         for each row execute function telegram.reject_repository_result_job()",
    )
    .execute(database.pool())
    .await
    .expect("result failure trigger");
}

async fn wait_for_atomic_result_fault(database: &TestDatabase) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let fired: bool =
                sqlx::query_scalar("select is_called from telegram.atomic_result_fault_sequence")
                    .fetch_one(database.pool())
                    .await
                    .expect("atomic result fault evidence");
            if fired {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("result storage fault was not reached");
}

async fn reject_next_repository_result_job(database: &TestDatabase) {
    sqlx::query("create sequence telegram.repository_result_fault_sequence start with 1")
        .execute(database.pool())
        .await
        .expect("result fault sequence");
    sqlx::query(
        "create function telegram.reject_next_repository_result_job() returns trigger
         language plpgsql as $$
         begin
             if new.kind = 'send_message'
                and new.payload->>'text' like '<b>Repository action%'
                and nextval('telegram.repository_result_fault_sequence') = 1 then
                 raise exception 'injected one-shot repository result storage failure';
             end if;
             return new;
         end
         $$",
    )
    .execute(database.pool())
    .await
    .expect("one-shot result failure function");
    sqlx::query(
        "create trigger reject_next_repository_result_job
         before insert on telegram.outbound_jobs
         for each row execute function telegram.reject_next_repository_result_job()",
    )
    .execute(database.pool())
    .await
    .expect("one-shot result failure trigger");
}

#[tokio::test]
async fn confirmed_action_result_completion_is_atomic() {
    let (base, calls) = harness(ActionAnswer::Success).await;
    let fixture = Fixture::create(&base).await;
    let (dialogue_id, confirm) = prepare_confirmation(&fixture, 13_001, 13_002).await;
    reject_repository_result_jobs(&fixture.database).await;

    fixture
        .deliver(callback(13_003, 101, &confirm, OWNER))
        .await;
    wait_for_atomic_result_fault(&fixture.database).await;
    assert!(calls.actions.load(Ordering::SeqCst) >= 1);

    let dialogue: (String, String, bool) = sqlx::query_as(
        "select step, lifecycle, payload ? 'result'
         from telegram.dialog_states where id = $1",
    )
    .bind(dialogue_id)
    .fetch_one(fixture.database.pool())
    .await
    .expect("dialogue state after result storage failure");
    assert_eq!(
        dialogue,
        ("submitting".to_owned(), "active".to_owned(), false),
        "completion must roll back with its rejected result job"
    );
    let result_jobs: i64 = sqlx::query_scalar(
        "select count(*)::bigint from telegram.outbound_jobs
         where payload->>'text' like '<b>Repository action%'",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("result job count");
    assert_eq!(result_jobs, 0, "the rejected result job must not exist");
}

#[tokio::test]
async fn confirmed_action_recovers_result_after_storage_failure() {
    let (base, calls) = harness(ActionAnswer::Success).await;
    let fixture = Fixture::create(&base).await;
    let (dialogue_id, confirm) = prepare_confirmation(&fixture, 14_001, 14_002).await;
    reject_next_repository_result_job(&fixture.database).await;

    fixture
        .deliver(callback(14_003, 101, &confirm, OWNER))
        .await;
    fixture.wait_settled(14_003).await;

    let update: (String, bool) = sqlx::query_as(
        "select state, payload is null from telegram.updates
         where bot_id = $1 and update_id = $2",
    )
    .bind(BOT)
    .bind(14_003_i64)
    .fetch_one(fixture.database.pool())
    .await
    .expect("recovered update state");
    assert_eq!(
        update,
        ("processed".to_owned(), true),
        "a post-confirmation storage fault must retain the update until recovery completes"
    );

    let completed_dialogues: i64 = sqlx::query_scalar(
        "select count(*)::bigint from telegram.dialog_states
         where id = $1 and step = 'completed' and lifecycle = 'completed'
           and payload ? 'result'",
    )
    .bind(dialogue_id)
    .fetch_one(fixture.database.pool())
    .await
    .expect("completed dialogue count");
    assert_eq!(completed_dialogues, 1, "one dialogue must complete");
    let result_jobs: i64 = sqlx::query_scalar(
        "select count(*)::bigint from telegram.outbound_jobs
         where payload->>'text' like '<b>Repository action%'",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("result job count");
    assert_eq!(result_jobs, 1, "one result projection must survive");

    assert_eq!(
        calls.actions.load(Ordering::SeqCst),
        2,
        "the original update must retry the uncertain accepted action once"
    );
    let keys = calls.action_keys.lock().expect("action keys").clone();
    assert_eq!(keys.len(), 2);
    assert!(
        keys.windows(2).all(|pair| pair[0] == pair[1]),
        "every Platform request must reuse the dialogue action identity: {keys:?}"
    );
}

#[tokio::test]
async fn repository_preview_confirmation_gate_and_partial_result_are_truthful() {
    let (base, calls) = harness(ActionAnswer::Success).await;
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

    let flow_id: uuid::Uuid = sqlx::query_scalar("select id from telegram.dialog_states")
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
    let (base, calls) = harness(ActionAnswer::Unavailable).await;
    let fixture = Fixture::create(&base).await;
    fixture.deliver(message(11_001)).await;
    fixture.wait_settled(11_001).await;
    let flow_id: uuid::Uuid = sqlx::query_scalar("select id from telegram.dialog_states")
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

    assert_eq!(
        calls.actions.load(Ordering::SeqCst),
        16,
        "eight durable claims each use the two-attempt HTTP budget"
    );
    let keys = calls.action_keys.lock().expect("keys").clone();
    assert_eq!(keys.len(), 16);
    assert!(
        keys.windows(2).all(|pair| pair[0] == pair[1]),
        "retries must reuse one identity"
    );
    let update: (String, bool) = sqlx::query_as(
        "select state, payload is not null from telegram.updates
         where bot_id = $1 and update_id = $2",
    )
    .bind(BOT)
    .bind(11_003_i64)
    .fetch_one(fixture.database.pool())
    .await
    .expect("bounded recovery state");
    assert_eq!(update, ("recovery_required".to_owned(), true));
    let result_jobs: i64 = sqlx::query_scalar(
        "select count(*)::bigint from telegram.outbound_jobs
         where payload->>'text' like '<b>Repository action%'",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("result projection count");
    assert_eq!(
        result_jobs, 0,
        "uncertainty must never be rendered as success"
    );
}

#[tokio::test]
async fn permanent_action_refusal_settles_once_with_safe_result() {
    let (base, calls) = harness(ActionAnswer::Refused).await;
    let fixture = Fixture::create(&base).await;
    let (dialogue_id, confirm) = prepare_confirmation(&fixture, 15_001, 15_002).await;

    fixture
        .deliver(callback(15_003, 101, &confirm, OWNER))
        .await;
    fixture.wait_settled(15_003).await;

    assert_eq!(calls.actions.load(Ordering::SeqCst), 1);
    let update: (String, bool) = sqlx::query_as(
        "select state, payload is null from telegram.updates
         where bot_id = $1 and update_id = $2",
    )
    .bind(BOT)
    .bind(15_003_i64)
    .fetch_one(fixture.database.pool())
    .await
    .expect("terminal update state");
    assert_eq!(update, ("processed".to_owned(), true));
    let dialogue: (String, String, bool) = sqlx::query_as(
        "select step, lifecycle, payload ? 'result'
         from telegram.dialog_states where id = $1",
    )
    .bind(dialogue_id)
    .fetch_one(fixture.database.pool())
    .await
    .expect("terminal dialogue");
    assert_eq!(
        dialogue,
        ("completed".to_owned(), "completed".to_owned(), false)
    );
    let result: String = sqlx::query_scalar(
        "select payload->>'text' from telegram.outbound_jobs
         where payload->>'text' like '<b>Repository action%'",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("safe refusal projection");
    assert!(result.contains("could not be accepted"), "{result}");
    assert!(!result.contains("succeeded"), "{result}");
}

#[tokio::test]
async fn second_press_is_answered_as_expired_without_another_action() {
    let (base, calls) = harness(ActionAnswer::Success).await;
    let fixture = Fixture::create(&base).await;
    fixture.deliver(message(12_001)).await;
    fixture.wait_settled(12_001).await;
    let flow_id: uuid::Uuid = sqlx::query_scalar("select id from telegram.dialog_states")
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
        .deliver(callback(12_002, 100, &select_star, OWNER))
        .await;
    fixture.wait_settled(12_002).await;
    fixture
        .deliver(callback(12_003, 100, &select_star, OWNER))
        .await;
    fixture.wait_settled(12_003).await;

    assert_eq!(calls.callback_answers.load(Ordering::SeqCst), 2);
    assert_eq!(calls.actions.load(Ordering::SeqCst), 0);
    let version: i64 = sqlx::query_scalar("select version from telegram.dialog_states")
        .fetch_one(fixture.database.pool())
        .await
        .expect("one selection transition");
    assert_eq!(version, 1, "the replay cannot advance the flow");
    let reply: String = sqlx::query_scalar(
        "select payload->>'text' from telegram.outbound_jobs order by id desc limit 1",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("expired-state reply");
    assert_eq!(reply, "This action has expired. Please start again.");
}
