//! Recovery invariants for the atomic accepted-capture projection.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use sha2::{Digest as _, Sha256};
use telegram_persistence::interaction_tokens::{
    NewOperationIntent, OperationIntentPayload, TokenScope,
};
use telegram_persistence::outbound_jobs::{MessagePayload, NewOutboundJob, OutboundJobKind};
use telegram_persistence::test_support::TestDatabase;

const BOT: i64 = 700_100_200;
const OWNER: i64 = 900_700_601;
const T0: i64 = 1_800_000_000;
const T1: i64 = T0 + 20;

fn scope() -> TokenScope {
    TokenScope {
        bot_id: BOT,
        telegram_user_id: OWNER,
        chat_id: OWNER,
        message_id: None,
    }
}

fn operation_intent(operation_id: uuid::Uuid, expires_at: i64) -> NewOperationIntent {
    NewOperationIntent {
        scope: scope(),
        operation_id,
        payload: OperationIntentPayload {
            source_url: Some("https://example.test/article".to_owned()),
            metadata: None,
        },
        expires_at,
    }
}

fn acknowledgement(operation_id: uuid::Uuid) -> NewOutboundJob {
    let payload = MessagePayload::text("Capturing");
    let canonical = payload
        .canonical()
        .expect("the fixture payload is canonical");
    NewOutboundJob {
        bot_id: BOT,
        chat_id: OWNER,
        kind: OutboundJobKind::SendMessage,
        payload,
        content_hash: format!("{:x}", Sha256::digest(canonical.as_bytes())),
        operation_id: Some(operation_id),
        revision: None,
        correlation_id: Some(format!("operation:{operation_id}")),
        next_attempt_at: None,
    }
}

/// Recovery after an expired presentation authority must atomically issue a fresh live intent.
#[tokio::test]
async fn expired_operation_intent_is_replaced_during_capture_projection_recovery() {
    let test = TestDatabase::create().await.expect("database");
    let operation_id = uuid::Uuid::now_v7();
    test.database
        .issue_operation_intent(&operation_intent(operation_id, T0 + 10), T0)
        .await
        .expect("the expired initial intent is stored");

    test.database
        .record_accepted_capture_projection(
            &operation_intent(operation_id, T1 + 60),
            &acknowledgement(operation_id),
            T1,
        )
        .await
        .expect("the accepted capture projection recovers");

    let live_intents: i64 = sqlx::query_scalar(
        "select count(*) from telegram.interaction_tokens
         where surface = 'deep_link' and action = 'operation_status'
           and bot_id = $1 and telegram_user_id = $2 and chat_id = $3 and operation_id = $4
           and consumed_at is null and expires_at > to_timestamp($5)",
    )
    .bind(BOT)
    .bind(OWNER)
    .bind(OWNER)
    .bind(operation_id)
    .bind(T1)
    .fetch_one(test.pool())
    .await
    .expect("live intent count");

    test.cleanup().await.expect("cleanup");
    assert_eq!(
        live_intents, 1,
        "recovery must leave one fresh unconsumed operation intent"
    );
}
