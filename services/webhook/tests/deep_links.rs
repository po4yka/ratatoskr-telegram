//! Opaque `/start` intent consumption through the real webhook worker.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use std::time::Duration;

use http::{Request, StatusCode};
use ratatoskr_telegram_webhook::intake::{self, Intake, IntakeSettings};
use secrecy::SecretString;
use serde_json::{Value, json};
use telegram_persistence::IdentityProfile;
use telegram_persistence::interaction_tokens::{
    NewOperationIntent, OperationIntentPayload, TokenScope,
};
use telegram_persistence::test_support::TestDatabase;
use tower::ServiceExt as _;

const BOT: i64 = 700_100_200;
const OWNER: i64 = 900_700_601;
const FOREIGN: i64 = 900_700_602;
const T0: i64 = 1_800_000_000;
const SECRET: &str = "webhook-secret-0123456789abcdef";

fn start_message(update_id: i64, actor: i64, token: &str) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id,
            "from": {"id": actor, "is_bot": false, "first_name": "Synthetic"},
            "date": T0,
            "chat": {"id": actor, "type": "private", "first_name": "Synthetic"},
            "text": format!("/start {token}")
        }
    })
}

async fn deliver(app: &axum::Router, update: Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("content-type", "application/json")
                .header("x-telegram-bot-api-secret-token", SECRET)
                .body(axum::body::Body::from(update.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
}

async fn wait_settled(database: &TestDatabase, update_id: i64) {
    for _ in 0..200 {
        let state = sqlx::query_scalar::<_, String>(
            "select state from telegram.updates where bot_id = $1 and update_id = $2",
        )
        .bind(BOT)
        .bind(update_id)
        .fetch_optional(database.pool())
        .await
        .expect("update state");
        if state.is_some_and(|state| !matches!(state.as_str(), "accepted" | "processing")) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("update {update_id} did not settle");
}

async fn token_consumer(database: &TestDatabase, token: &str) -> Option<i64> {
    sqlx::query_scalar("select consumed_by_user from telegram.interaction_tokens where token = $1")
        .bind(token)
        .fetch_one(database.pool())
        .await
        .expect("token consumer")
}

#[tokio::test]
async fn valid_start_token_is_consumed_but_replay_and_foreign_scope_release_nothing() {
    let database = TestDatabase::create().await.expect("database");
    for actor in [OWNER, FOREIGN] {
        database
            .database
            .ensure_identity(actor, &IdentityProfile::default())
            .await
            .expect("enabled identity");
    }
    let operation_id = uuid::Uuid::now_v7();
    database
        .database
        .ensure_operation_binding(BOT, operation_id, OWNER)
        .await
        .expect("operation binding");
    let token = database
        .database
        .issue_operation_intent(
            &NewOperationIntent {
                scope: TokenScope {
                    bot_id: BOT,
                    telegram_user_id: OWNER,
                    chat_id: OWNER,
                    message_id: None,
                },
                operation_id,
                payload: OperationIntentPayload {
                    source_url: Some("https://example.test/article".to_owned()),
                    metadata: None,
                },
                expires_at: T0 + 900,
            },
            T0,
        )
        .await
        .expect("deep-link intent");
    let before: (i64, bool, Option<i64>) = sqlx::query_as(
        "select count(*) over (), terminal, message_id
         from telegram.message_bindings where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(database.pool())
    .await
    .expect("binding snapshot");

    let (intake, receiver) = Intake::new(
        IntakeSettings {
            secret: SecretString::new(SECRET.into()),
            max_body_bytes: 4096,
            bot_id: BOT,
            queue_capacity: 16,
        },
        database.database.clone(),
    );
    let app = intake.router();
    let worker = tokio::spawn(intake::run_worker(
        database.database.clone(),
        receiver,
        None,
    ));

    deliver(&app, start_message(40_001, FOREIGN, &token)).await;
    wait_settled(&database, 40_001).await;
    assert_eq!(token_consumer(&database, &token).await, None);

    deliver(&app, start_message(40_002, OWNER, &token)).await;
    wait_settled(&database, 40_002).await;
    assert_eq!(token_consumer(&database, &token).await, Some(OWNER));

    deliver(&app, start_message(40_003, OWNER, &token)).await;
    wait_settled(&database, 40_003).await;
    let after: (i64, bool, Option<i64>) = sqlx::query_as(
        "select count(*) over (), terminal, message_id
         from telegram.message_bindings where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(database.pool())
    .await
    .expect("binding remains");
    assert_eq!(
        after, before,
        "intent resolution cannot mutate domain projection state"
    );
    let token_rows: i64 = sqlx::query_scalar(
        "select count(*) from telegram.interaction_tokens where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(database.pool())
    .await
    .expect("token count");
    assert_eq!(token_rows, 1, "replay cannot mint replacement authority");
    worker.abort();
}
