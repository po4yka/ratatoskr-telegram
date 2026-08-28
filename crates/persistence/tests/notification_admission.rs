//! Transactional notification policy, quiet-hours deferral, and logical-id deduplication.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use telegram_persistence::bindings::IdentityProfile;
use telegram_persistence::notification_delivery::{
    NewNotificationDelivery, NotificationAdmissionResult, NotificationDecisionOutcome,
};
use telegram_persistence::outbound_jobs::DeliveryOutcome;
use telegram_persistence::outbound_jobs::MessagePayload;
use telegram_persistence::test_support::TestDatabase;

const BOT: i64 = 700_100_200;
const CHAT: i64 = 900_700_600;
const AT_2300_UTC: i64 = 86_400 * 20_000 + 23 * 3_600;

async fn fixture() -> (TestDatabase, uuid::Uuid) {
    let test = TestDatabase::create().await.expect("disposable database");
    let internal_user = uuid::Uuid::now_v7();
    test.database
        .ensure_identity(BOT, &IdentityProfile::default())
        .await
        .expect("identity");
    sqlx::query("update telegram.identities set internal_user_id = $1 where telegram_user_id = $2")
        .bind(internal_user)
        .bind(BOT)
        .execute(test.pool())
        .await
        .expect("identity binding");
    test.database.ensure_chat(CHAT).await.expect("chat");
    test.database
        .bind_private_chat(BOT, CHAT)
        .await
        .expect("private binding");
    (test, internal_user)
}

fn notification(internal_user: uuid::Uuid, class: &str, now: i64) -> NewNotificationDelivery {
    NewNotificationDelivery {
        bot_id: BOT,
        event_id: uuid::Uuid::now_v7(),
        stream_sequence: None,
        notification_id: uuid::Uuid::now_v7(),
        recipient_user_id: internal_user,
        class: class.to_owned(),
        priority_high: false,
        quiet_hint_seconds: None,
        payload: MessagePayload {
            text: "<b>Safe &amp; bounded</b>".to_owned(),
            parse_mode: Some("HTML".to_owned()),
            reply_markup: None,
        },
        correlation_id: Some("operation:018f0000-0000-7000-8000-000000000302".to_owned()),
        occurred_at: now,
    }
}

#[tokio::test]
async fn notification_admission_enforces_policy_and_quiet_hours() {
    let (test, user) = fixture().await;
    let enabled = notification(user, "operation_completed", AT_2300_UTC);
    let result = test
        .database
        .admit_notification(&enabled, AT_2300_UTC)
        .await
        .expect("enabled admission");
    assert_eq!(
        result,
        NotificationAdmissionResult::Decided(vec![NotificationDecisionOutcome::Enqueued])
    );

    sqlx::query(
        "update telegram.notification_preferences
         set quiet_policy = 'custom', quiet_start_minute = 1320, quiet_end_minute = 420,
             version = version + 1 where telegram_user_id = $1 and chat_id = $2",
    )
    .bind(BOT)
    .bind(CHAT)
    .execute(test.pool())
    .await
    .expect("quiet policy");
    let deferred = notification(user, "analysis_ready", AT_2300_UTC);
    assert_eq!(
        test.database
            .admit_notification(&deferred, AT_2300_UTC)
            .await
            .expect("deferred admission"),
        NotificationAdmissionResult::Decided(vec![NotificationDecisionOutcome::Deferred])
    );

    sqlx::query(
        "update telegram.notification_preferences
         set high_priority_bypass = true where telegram_user_id = $1 and chat_id = $2",
    )
    .bind(BOT)
    .bind(CHAT)
    .execute(test.pool())
    .await
    .expect("bypass policy");
    let mut high = notification(user, "backup_outcome", AT_2300_UTC);
    high.priority_high = true;
    assert_eq!(
        test.database
            .admit_notification(&high, AT_2300_UTC)
            .await
            .expect("high admission"),
        NotificationAdmissionResult::Decided(vec![NotificationDecisionOutcome::Enqueued])
    );

    sqlx::query(
        "insert into telegram.notification_class_preferences
             (telegram_user_id, chat_id, class, enabled)
         values ($1, $2, 'watch_triggered', false)",
    )
    .bind(BOT)
    .bind(CHAT)
    .execute(test.pool())
    .await
    .expect("class override");
    let suppressed = notification(user, "watch_triggered", AT_2300_UTC);
    assert_eq!(
        test.database
            .admit_notification(&suppressed, AT_2300_UTC)
            .await
            .expect("suppressed admission"),
        NotificationAdmissionResult::Decided(vec![NotificationDecisionOutcome::Suppressed])
    );

    // Unknown well-formed classes inherit the global toggle and remain deliverable.
    let unknown = notification(user, "future_class", AT_2300_UTC + 9 * 3_600);
    assert!(matches!(
        test.database
            .admit_notification(&unknown, AT_2300_UTC + 9 * 3_600)
            .await
            .expect("unknown class"),
        NotificationAdmissionResult::Decided(_)
    ));
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn notification_identity_deduplicates_distinct_event_envelopes() {
    let (test, user) = fixture().await;
    let first = notification(user, "operation_failed", AT_2300_UTC);
    let mut replay = first.clone();
    replay.event_id = uuid::Uuid::now_v7();
    test.database
        .admit_notification(&first, AT_2300_UTC)
        .await
        .expect("first");
    assert_eq!(
        test.database
            .admit_notification(&replay, AT_2300_UTC)
            .await
            .expect("replay"),
        NotificationAdmissionResult::DuplicateNotification
    );
    let jobs: i64 = sqlx::query_scalar(
        "select count(*) from telegram.outbound_jobs where notification_id = $1",
    )
    .bind(first.notification_id)
    .fetch_one(test.pool())
    .await
    .expect("job count");
    assert_eq!(jobs, 1);
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn concurrent_notification_admission_creates_one_job() {
    let (test, user) = fixture().await;
    let notification = notification(user, "archive_imported", AT_2300_UTC);
    let left = test.database.clone();
    let right = test.database.clone();
    let left_notification = notification.clone();
    let right_notification = notification.clone();
    let (left_result, right_result) = tokio::join!(
        left.admit_notification(&left_notification, AT_2300_UTC),
        right.admit_notification(&right_notification, AT_2300_UTC)
    );
    assert!(left_result.is_ok() && right_result.is_ok());
    let jobs: i64 = sqlx::query_scalar(
        "select count(*) from telegram.outbound_jobs where notification_id = $1",
    )
    .bind(notification.notification_id)
    .fetch_one(test.pool())
    .await
    .expect("job count");
    assert_eq!(jobs, 1);
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn no_eligible_chat_sends_nothing() {
    let test = TestDatabase::create().await.expect("disposable database");
    let mut event = notification(uuid::Uuid::now_v7(), "analysis_ready", AT_2300_UTC);
    event.stream_sequence = Some(77);
    assert_eq!(
        test.database
            .admit_notification(&event, AT_2300_UTC)
            .await
            .expect("no eligible chat"),
        NotificationAdmissionResult::NoEligibleChat
    );
    let jobs: i64 = sqlx::query_scalar("select count(*) from telegram.outbound_jobs")
        .fetch_one(test.pool())
        .await
        .expect("job count");
    assert_eq!(jobs, 0);
    let evidence: (i64, uuid::Uuid, String) = sqlx::query_as(
        "select stream_sequence, event_id, failure_class
         from telegram.notification_transport_failures",
    )
    .fetch_one(test.pool())
    .await
    .expect("content-free transport evidence");
    assert_eq!(
        evidence,
        (77, event.event_id, "invalid_notification".to_owned())
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn sender_settlement_updates_the_linked_notification_decision() {
    let (test, user) = fixture().await;
    let event = notification(user, "operation_completed", AT_2300_UTC);
    test.database
        .admit_notification(&event, AT_2300_UTC)
        .await
        .expect("admission");
    let job = test
        .database
        .claim_due_outbound_job(AT_2300_UTC, 60)
        .await
        .expect("claim")
        .expect("notification job");
    let linked_job: Option<uuid::Uuid> = sqlx::query_scalar(
        "select outbound_job_id from telegram.notification_decisions where notification_id = $1",
    )
    .bind(event.notification_id)
    .fetch_one(test.pool())
    .await
    .expect("decision link");
    assert_eq!(linked_job, Some(job.id));
    test.database
        .settle_outbound_job(job.id, AT_2300_UTC + 1, 5, &DeliveryOutcome::Sent)
        .await
        .expect("settlement");
    let outcome: String = sqlx::query_scalar(
        "select outcome from telegram.notification_decisions where notification_id = $1",
    )
    .bind(event.notification_id)
    .fetch_one(test.pool())
    .await
    .expect("decision");
    assert_eq!(outcome, "delivered");
    test.cleanup().await.expect("cleanup");
}
