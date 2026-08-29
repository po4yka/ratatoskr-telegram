//! Unknown send quarantine, acknowledgement atomicity, and claim fencing.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test assertions")]

use telegram_persistence::PersistenceError;
use telegram_persistence::outbound_jobs::{
    AcknowledgedMethod, DeliveryOutcome, MessagePayload, NewOutboundJob, OutboundJobKind,
};
use telegram_persistence::test_support::TestDatabase;

const T0: i64 = 1_800_000_000;
const BOT: i64 = 700_100_200;
const CHAT: i64 = 900_700_610;

async fn database() -> TestDatabase {
    TestDatabase::create().await.expect("database")
}

fn job(kind: OutboundJobKind, operation_id: Option<uuid::Uuid>) -> NewOutboundJob {
    NewOutboundJob {
        bot_id: BOT,
        chat_id: CHAT,
        kind,
        payload: MessagePayload::text("body"),
        content_hash: "hash".to_owned(),
        operation_id,
        revision: operation_id.map(|_| 7),
        correlation_id: None,
        next_attempt_at: None,
    }
}

async fn state(db: &telegram_persistence::Database, id: uuid::Uuid) -> String {
    sqlx::query_scalar("select state from telegram.outbound_jobs where id = $1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .expect("job row")
}

#[tokio::test]
async fn expired_send_with_unknown_outcome_is_quarantined_without_a_second_wire_call() {
    let test = database().await;
    let id = test
        .database
        .enqueue_outbound_job(&job(OutboundJobKind::SendMessage, None), T0)
        .await
        .expect("enqueue");
    test.database
        .claim_due_outbound_job(T0, 30)
        .await
        .unwrap()
        .unwrap();
    assert!(
        test.database
            .claim_due_outbound_job(T0 + 31, 30)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(state(&test.database, id).await, "outcome_unknown");
}

#[tokio::test]
async fn expired_edit_remains_reclaimable() {
    let test = database().await;
    let id = test
        .database
        .enqueue_outbound_job(
            &job(OutboundJobKind::EditMessageText, Some(uuid::Uuid::now_v7())),
            T0,
        )
        .await
        .unwrap();
    test.database
        .claim_due_outbound_job(T0, 30)
        .await
        .unwrap()
        .unwrap();
    let reclaimed = test
        .database
        .claim_due_outbound_job(T0 + 31, 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((reclaimed.id, reclaimed.attempts), (id, 2));
}

#[tokio::test]
async fn stale_worker_cannot_settle_a_reclaimed_edit_attempt() {
    let test = database().await;
    let id = test
        .database
        .enqueue_outbound_job(
            &job(OutboundJobKind::EditMessageText, Some(uuid::Uuid::now_v7())),
            T0,
        )
        .await
        .unwrap();
    let worker_a = test
        .database
        .claim_due_outbound_job(T0, 30)
        .await
        .unwrap()
        .unwrap();
    let worker_b = test
        .database
        .claim_due_outbound_job(T0 + 31, 30)
        .await
        .unwrap()
        .unwrap();
    let stale = test
        .database
        .settle_outbound_job(id, worker_a.attempts, T0 + 32, 3, &DeliveryOutcome::Sent)
        .await;
    assert!(matches!(stale, Err(PersistenceError::StaleOutboundClaim)));
    assert_eq!(state(&test.database, id).await, "sending");
    test.database
        .settle_outbound_job(id, worker_b.attempts, T0 + 33, 3, &DeliveryOutcome::Sent)
        .await
        .unwrap();
    assert_eq!(state(&test.database, id).await, "sent");
}

#[tokio::test]
async fn acknowledged_delivery_updates_job_binding_and_revision_atomically() {
    let test = database().await;
    let operation = uuid::Uuid::now_v7();
    let id = test
        .database
        .enqueue_outbound_job(&job(OutboundJobKind::SendMessage, Some(operation)), T0)
        .await
        .unwrap();
    let claimed = test
        .database
        .claim_due_outbound_job(T0, 30)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "alter table telegram.outbound_jobs add constraint reject_sent_ack_for_test
         check (state <> 'sent')",
    )
    .execute(test.pool())
    .await
    .unwrap();
    let result = test
        .database
        .record_outbound_acknowledgement(&claimed, AcknowledgedMethod::SendMessage, 1000, T0 + 1)
        .await;
    assert!(result.is_err());
    assert!(
        test.database
            .find_binding(operation, CHAT)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(state(&test.database, id).await, "sending");
}
