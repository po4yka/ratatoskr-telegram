//! Notification scheduling and bounded aging in the shared outbound queue.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use telegram_persistence::outbound_jobs::DeliveryOutcome;
use telegram_persistence::test_support::TestDatabase;

const T0: i64 = 1_800_000_000;

#[tokio::test]
async fn direct_jobs_precede_new_notifications_and_old_notifications_age_in() {
    let test = TestDatabase::create().await.expect("disposable database");
    let db = &test.database;
    let notification = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into telegram.outbound_jobs
             (id, bot_id, chat_id, kind, payload, content_hash, delivery_class,
              notification_id, notification_created_at, next_attempt_at, created_at, updated_at)
         values ($1, 1, 10, 'send_message', '{\"text\":\"notification\"}', 'n',
                 'notification', $2, to_timestamp($3), to_timestamp($3), to_timestamp($3),
                 to_timestamp($3))",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(notification)
    .bind(T0)
    .execute(test.pool())
    .await
    .expect("notification job");
    sqlx::query(
        "insert into telegram.outbound_jobs
             (id, bot_id, chat_id, kind, payload, content_hash, delivery_class,
              next_attempt_at, created_at, updated_at)
         values ($1, 1, 10, 'send_message', '{\"text\":\"direct\"}', 'd', 'direct',
                 to_timestamp($2), to_timestamp($2), to_timestamp($2))",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(T0 + 1)
    .execute(test.pool())
    .await
    .expect("direct job");

    let first = db
        .claim_due_outbound_job(T0 + 1, 30)
        .await
        .expect("claim")
        .expect("direct claim");
    assert_eq!(first.payload.text, "direct");
    db.settle_outbound_job(first.id, first.attempts, T0 + 1, 5, &DeliveryOutcome::Sent)
        .await
        .expect("settle direct");
    let fresh_notification = db
        .claim_due_outbound_job(T0 + 1, 30)
        .await
        .expect("claim")
        .expect("notification claim");
    assert_eq!(fresh_notification.payload.text, "notification");

    db.settle_outbound_job(
        fresh_notification.id,
        fresh_notification.attempts,
        T0 + 1,
        5,
        &DeliveryOutcome::RetryWithBackoff { delay_secs: 1 },
    )
    .await
    .expect("reschedule notification");
    let aged = db
        .claim_due_outbound_job(T0 + 301, 30)
        .await
        .expect("claim")
        .expect("aged notification");
    assert_eq!(aged.payload.text, "notification");
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn deferred_notification_becomes_claimable_once_at_release() {
    let test = TestDatabase::create().await.expect("disposable database");
    sqlx::query(
        "insert into telegram.outbound_jobs
             (id, bot_id, chat_id, kind, payload, content_hash, delivery_class,
              notification_id, notification_created_at, next_attempt_at)
         values ($1, 1, 11, 'send_message', '{\"text\":\"deferred\"}', 'n',
                 'notification', $2, to_timestamp($3), to_timestamp($4))",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(T0)
    .bind(T0 + 60)
    .execute(test.pool())
    .await
    .expect("deferred job");
    assert!(
        test.database
            .claim_due_outbound_job(T0 + 59, 30)
            .await
            .expect("early claim")
            .is_none()
    );
    assert!(
        test.database
            .claim_due_outbound_job(T0 + 60, 30)
            .await
            .expect("boundary claim")
            .is_some()
    );
    assert!(
        test.database
            .claim_due_outbound_job(T0 + 61, 30)
            .await
            .expect("second claim")
            .is_none(),
        "one in-flight job per chat prevents duplicate delivery"
    );
    test.cleanup().await.expect("cleanup");
}
