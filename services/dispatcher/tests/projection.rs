//! The projection consumer against a real database: guard outcomes through the public accept
//! seam, and the burst-throttling property that keeps a progress storm off the wire.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

mod common;

use std::sync::Arc;

use common::{FakeClock, database};
use ratatoskr_telegram_dispatcher::outbound::DeliveryLimiter;
use ratatoskr_telegram_dispatcher::projection::{
    AcceptOutcome, OperationEvent, OperationStatus, ProjectionConsumer, SafeLine,
};
use sqlx::types::Uuid;
use telegram_persistence::test_support::TestDatabase;

/// A fixed synthetic instant: whole seconds since the Unix epoch, never read from a wall clock.
const T0: i64 = 1_800_000_000;
/// The render interval every test throttles against, seconds.
const INTERVAL: i64 = 4;
/// The one bot every synthetic row belongs to.
const BOT_ID: i64 = 700_100_200;

/// A running-stage event with one of each optional field; tests override what they vary.
fn an_event(operation: Uuid, event_id: Uuid, occurred_at: i64) -> OperationEvent {
    OperationEvent {
        event_id,
        occurred_at_secs: occurred_at,
        correlation_id: "operation:018f0000-0000-7000-8000-000000000001".to_owned(),
        operation_id: operation,
        status: OperationStatus::Running,
        stage: Some("downloading".to_owned()),
        progress_percent: Some(40),
        errors: Vec::new(),
        warnings: vec![SafeLine {
            code: "w.tick".to_owned(),
            message: "tick".to_owned(),
        }],
        message: None,
    }
}

/// A consumer over the shared database with its clock frozen at [`T0`].
fn consumer(db: &TestDatabase) -> ProjectionConsumer {
    ProjectionConsumer::new(
        db.database.clone(),
        FakeClock::at(T0),
        u64::try_from(INTERVAL).unwrap_or(0),
        None,
    )
}

/// Create the binding for `(operation, chat)` with message id 42 acknowledged at [`T0`].
async fn seed_binding(db: &TestDatabase, operation: Uuid, chat_id: i64) {
    db.database
        .ensure_operation_binding(BOT_ID, operation, chat_id)
        .await
        .expect("ensure binding");
    db.database
        .record_send_acknowledged(BOT_ID, operation, chat_id, 42, T0)
        .await
        .expect("ack");
}

/// `(state, count)` for every job state of one operation.
async fn job_counts(db: &TestDatabase, operation: Uuid) -> Vec<(String, i64)> {
    sqlx::query_as(
        "select state, count(*)::bigint
         from telegram.outbound_jobs
         where operation_id = $1
         group by state
         order by state",
    )
    .bind(operation)
    .fetch_all(db.pool())
    .await
    .expect("job counts")
}

async fn inbox_count(db: &TestDatabase) -> i64 {
    sqlx::query_scalar("select count(*)::bigint from telegram.inbox")
        .fetch_one(db.pool())
        .await
        .expect("inbox count")
}

/// The same envelope delivered twice is accepted once and reported duplicate once.
#[tokio::test]
async fn duplicate_envelope_event_id_is_dropped_and_counted() {
    let db = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&db, operation, 100).await;
    let consumer = consumer(&db);

    let event = an_event(operation, Uuid::now_v7(), T0 + 1);
    assert_eq!(
        consumer.accept(&event).await.expect("first accept"),
        AcceptOutcome::Recorded
    );
    assert_eq!(
        consumer.accept(&event).await.expect("second accept"),
        AcceptOutcome::Duplicate,
        "the redelivered envelope changes nothing twice"
    );
}

/// After a terminal render is accepted, later events — terminal or not — are dropped as
/// post-terminal, every time.
#[tokio::test]
async fn post_terminal_events_are_dropped_exactly_once_counted() {
    let db = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&db, operation, 200).await;
    let consumer = consumer(&db);

    let mut done = an_event(operation, Uuid::now_v7(), T0 + 1);
    done.status = OperationStatus::Succeeded;
    assert_eq!(
        consumer.accept(&done).await.expect("terminal"),
        AcceptOutcome::Recorded
    );

    let mut after_one = an_event(operation, Uuid::now_v7(), T0 + 2);
    after_one.status = OperationStatus::Failed;
    assert_eq!(
        consumer
            .accept(&after_one)
            .await
            .expect("first post-terminal"),
        AcceptOutcome::PostTerminal
    );

    let mut after_two = an_event(operation, Uuid::now_v7(), T0 + 3);
    after_two.status = OperationStatus::Running;
    assert_eq!(
        consumer
            .accept(&after_two)
            .await
            .expect("second post-terminal"),
        AcceptOutcome::PostTerminal,
        "every post-terminal arrival drops, not just the first"
    );

    assert_eq!(
        job_counts(&db, operation).await,
        vec![("ready".to_owned(), 1)],
        "only the terminal render was ever enqueued"
    );
}

/// An event older than the newest accepted one produces no job and moves nothing.
#[tokio::test]
async fn stale_occurred_at_is_dropped_without_effect() {
    let db = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&db, operation, 300).await;
    let consumer = consumer(&db);

    assert_eq!(
        consumer
            .accept(&an_event(operation, Uuid::now_v7(), T0 + 10))
            .await
            .expect("newest"),
        AcceptOutcome::Recorded
    );
    assert_eq!(
        consumer
            .accept(&an_event(operation, Uuid::now_v7(), T0 + 9))
            .await
            .expect("stale"),
        AcceptOutcome::Stale
    );

    assert_eq!(
        job_counts(&db, operation).await,
        vec![("ready".to_owned(), 1)],
        "the stale event enqueued nothing"
    );
}

/// An operation with no binding produces no traffic and leaves no trace at all.
#[tokio::test]
async fn unbound_operation_produces_no_traffic() {
    let db = database().await;
    let consumer = consumer(&db);

    let outcome = consumer
        .accept(&an_event(Uuid::now_v7(), Uuid::now_v7(), T0))
        .await
        .expect("accept");
    assert_eq!(outcome, AcceptOutcome::Unbound);

    assert_eq!(
        inbox_count(&db).await,
        0,
        "not even dedup evidence is written"
    );
    let jobs: Vec<(String, i64)> =
        sqlx::query_as("select state, count(*)::bigint from telegram.outbound_jobs group by state")
            .fetch_all(db.pool())
            .await
            .expect("all jobs");
    assert!(jobs.is_empty(), "no outbound traffic for the unbound");
}

/// Ten progress ticks inside one second enqueue ten jobs — each tick is newer, so none is stale —
/// but the supersede sweep collapses every predecessor, so exactly ONE edit is ever eligible, it
/// becomes eligible only when the interval window past the last DELIVERED render opens, and the
/// claim's one-in-flight rule keeps it single.
///
/// Pinned here: (a) no newer tick is dropped at accept; (b) superseded intermediates can never
/// reach the wire; (c) durable throttle arithmetic gates eligibility on the delivered-render
/// anchor, not on accept time; (d) exactly the newest revision survives to be claimed.
#[tokio::test]
async fn progress_burst_yields_at_most_one_eligible_edit_per_interval() {
    let db = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&db, operation, 400).await;
    // A delivered render anchors the throttle window: last_rendered_at = T0. Revision 1 because
    // the column defaults to 0 and the guarded advance refuses anything not strictly newer.
    db.database
        .advance_render(operation, 400, 1, T0)
        .await
        .expect("anchor render");

    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    let _ = limiter; // the sender owns the gate in group 3; here the queue's own arithmetic is under test

    let consumer = consumer(&db);
    for tick in 1..=10i64 {
        let outcome = consumer
            .accept(&an_event(operation, Uuid::now_v7(), T0 + tick))
            .await
            .expect("accept");
        assert!(
            matches!(outcome, AcceptOutcome::Recorded),
            "tick {tick} is newer than everything before it and must be recorded"
        );
    }

    assert_eq!(
        job_counts(&db, operation).await,
        vec![("ready".to_owned(), 1), ("superseded".to_owned(), 9)],
        "ten jobs enqueued, nine swept: intermediates never reach the wire"
    );

    // Before the window opens nothing is claimable, even though accepts happened "now".
    assert!(
        db.database
            .claim_due_outbound_job(T0 + INTERVAL - 1, 30)
            .await
            .expect("claim")
            .is_none(),
        "the durable interval gates eligibility"
    );

    // When it opens, exactly the newest revision comes out — once. The anchor consumed
    // revision 1, so the ten ticks were assigned 2..=11 and revision 11 survives.
    let claimed = db
        .database
        .claim_due_outbound_job(T0 + INTERVAL, 30)
        .await
        .expect("claim")
        .expect("the surviving edit is due");
    assert_eq!(
        claimed.revision,
        Some(11),
        "the newest revision wins the burst"
    );

    assert!(
        db.database
            .claim_due_outbound_job(T0 + INTERVAL, 30)
            .await
            .expect("claim")
            .is_none(),
        "one job in flight per chat: the burst yields exactly one delivery"
    );
}
