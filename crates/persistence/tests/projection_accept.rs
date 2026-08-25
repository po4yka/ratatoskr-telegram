//! The projection accept step: one transactional guard sequence per operation event, against a
//! disposable database per test.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use sqlx::types::Uuid;
use telegram_persistence::message_bindings::MessageBindingRecord;
use telegram_persistence::projection_accept::{AcceptOutcome, AcceptedEvent};
use telegram_persistence::test_support::TestDatabase;

/// A fixed synthetic instant: whole seconds since the Unix epoch, never read from a wall clock.
const T0: i64 = 1_800_000_000;
/// The render interval the tests throttle against, seconds.
const INTERVAL: i64 = 4;
/// The one bot every synthetic row belongs to.
const BOT_ID: i64 = 700_100_200;

/// A disposable database per test.
async fn database() -> TestDatabase {
    TestDatabase::create()
        .await
        .expect("a disposable database must be creatable")
}

/// An event with all fields filled; tests override what they vary.
fn an_event(operation: Uuid, event_id: Uuid, occurred_at: i64) -> AcceptedEvent<'static> {
    AcceptedEvent {
        operation_id: operation,
        event_id,
        occurred_at_secs: occurred_at,
        terminal: false,
        body: "rendered progress",
        content_hash: "hash-of-rendered-progress",
        correlation_id: "operation:018f0000-0000-7000-8000-000000000001",
    }
}

/// `(state, next_attempt_epoch)` of one job row.
async fn job_shape(db: &TestDatabase, id: Uuid) -> (String, Option<i64>) {
    sqlx::query_as(
        "select state, extract(epoch from next_attempt_at)::bigint
         from telegram.outbound_jobs
         where id = $1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .expect("job row")
}

/// How many jobs exist for one operation, by state.
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

/// A non-terminal accept records the inbox row, assigns revision `last + 1`, and enqueues an edit
/// whose earliest attempt honors the render interval anchored at the last DELIVERED render — due
/// immediately when nothing was rendered yet, throttled past `now` when it was (design D4).
#[tokio::test]
async fn accept_records_first_event_and_enqueues_throttled_edit() {
    let test = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&test, operation, 100).await;

    let outcome = test
        .database
        .accept_operation_event(
            an_event(operation, Uuid::now_v7(), T0 + 10),
            T0 + 10,
            INTERVAL,
        )
        .await
        .expect("accept");
    let AcceptOutcome::Recorded { revision } = outcome else {
        panic!("the first event must be recorded, got {outcome:?}");
    };
    assert_eq!(revision, 1, "revisions start at last_rendered_revision + 1");

    // No render has been delivered yet, so the job is due at once.
    let jobs = job_counts(&test, operation).await;
    assert_eq!(
        jobs,
        vec![("ready".to_owned(), 1)],
        "exactly one ready edit"
    );

    // A delivered render anchors the interval: the same-shaped accept now waits past `now`.
    test.database
        .advance_render(operation, 100, 1, T0 + 20)
        .await
        .expect("advance");
    let outcome = test
        .database
        .accept_operation_event(
            an_event(operation, Uuid::now_v7(), T0 + 21),
            T0 + 21,
            INTERVAL,
        )
        .await
        .expect("accept");
    let AcceptOutcome::Recorded { revision } = outcome else {
        panic!("a newer event must be recorded, got {outcome:?}");
    };
    assert_eq!(revision, 2);

    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "select id from telegram.outbound_jobs where operation_id = $1 and state = 'ready'",
    )
    .bind(operation)
    .fetch_all(test.pool())
    .await
    .expect("ready rows");
    let (_, next_attempt) = job_shape(&test, rows[0].0).await;
    assert_eq!(
        next_attempt,
        Some(T0 + 20 + INTERVAL),
        "throttle anchors at the last DELIVERED render, not at now"
    );
}

/// The first terminal event flips the flag and enqueues an immediate job; every later event for
/// the binding is dropped as post-terminal, and exactly one job exists across the lifetime.
#[tokio::test]
async fn accept_terminal_sets_flag_inserts_immediate_job_once() {
    let test = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&test, operation, 200).await;

    let mut terminal = an_event(operation, Uuid::now_v7(), T0 + 1);
    terminal.terminal = true;
    let outcome = test
        .database
        .accept_operation_event(terminal, T0 + 1, INTERVAL)
        .await
        .expect("accept");
    assert!(
        matches!(outcome, AcceptOutcome::Recorded { revision: 1 }),
        "the first terminal event is recorded, got {outcome:?}"
    );

    let binding = binding_of(&test, operation).await;
    assert!(binding.terminal, "the flag flips on acceptance");

    let (_, next_attempt) = job_shape(
        &test,
        sqlx::query_scalar("select id from telegram.outbound_jobs where operation_id = $1")
            .bind(operation)
            .fetch_one(test.pool())
            .await
            .expect("one job"),
    )
    .await;
    assert_eq!(
        next_attempt,
        Some(T0 + 1),
        "terminals skip the interval delay"
    );

    let mut late = an_event(operation, Uuid::now_v7(), T0 + 2);
    late.terminal = true;
    let outcome = test
        .database
        .accept_operation_event(late, T0 + 2, INTERVAL)
        .await
        .expect("accept");
    assert_eq!(outcome, AcceptOutcome::PostTerminal);

    assert_eq!(
        job_counts(&test, operation).await,
        vec![("ready".to_owned(), 1)],
        "exactly one job ever exists for the binding"
    );
}

/// An event older than the newest ACCEPTED one is dropped without enqueuing anything or moving
/// the watermark; its envelope stays recorded so redeliveries short-circuit.
#[tokio::test]
async fn accept_stale_occurred_at_is_dropped_without_effect() {
    let test = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&test, operation, 300).await;

    test.database
        .accept_operation_event(
            an_event(operation, Uuid::now_v7(), T0 + 10),
            T0 + 10,
            INTERVAL,
        )
        .await
        .expect("first accept");

    let outcome = test
        .database
        .accept_operation_event(
            an_event(operation, Uuid::now_v7(), T0 + 9),
            T0 + 11,
            INTERVAL,
        )
        .await
        .expect("second accept");
    assert_eq!(outcome, AcceptOutcome::Stale);

    assert_eq!(
        job_counts(&test, operation).await,
        vec![("ready".to_owned(), 1)],
        "a stale event enqueues nothing"
    );
    assert_eq!(
        binding_of(&test, operation).await.last_event_at,
        Some(T0 + 10),
        "a stale drop never moves the watermark"
    );
}

/// Redelivering the same envelope id is a no-op the second time.
#[tokio::test]
async fn accept_duplicate_envelope_is_dropped() {
    let test = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&test, operation, 400).await;

    let event_id = Uuid::now_v7();
    let first = test
        .database
        .accept_operation_event(an_event(operation, event_id, T0 + 1), T0 + 1, INTERVAL)
        .await
        .expect("first accept");
    assert!(matches!(first, AcceptOutcome::Recorded { .. }));

    let second = test
        .database
        .accept_operation_event(an_event(operation, event_id, T0 + 1), T0 + 2, INTERVAL)
        .await
        .expect("second accept");
    assert_eq!(second, AcceptOutcome::Duplicate);

    assert_eq!(
        job_counts(&test, operation).await,
        vec![("ready".to_owned(), 1)],
        "a duplicate enqueues nothing"
    );
}

/// An event whose operation has no binding writes NOTHING — not even the inbox row — so a bind
/// landing later can still consume earlier redeliveries.
#[tokio::test]
async fn accept_unbound_records_nothing() {
    let test = database().await;

    let outcome = test
        .database
        .accept_operation_event(an_event(Uuid::now_v7(), Uuid::now_v7(), T0), T0, INTERVAL)
        .await
        .expect("accept");
    assert_eq!(outcome, AcceptOutcome::Unbound);

    assert_eq!(
        inbox_count(&test).await,
        0,
        "no dedup evidence for the unbound"
    );
    let jobs: Vec<(String, i64)> =
        sqlx::query_as("select state, count(*)::bigint from telegram.outbound_jobs group by state")
            .fetch_all(test.pool())
            .await
            .expect("all jobs");
    assert!(jobs.is_empty(), "no traffic for the unbound");
}

/// The watermark advances only when an event is accepted: duplicates and stale drops leave it,
/// a newer acceptance moves it.
#[tokio::test]
async fn watermark_advances_only_on_accepted_events() {
    let test = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&test, operation, 500).await;

    test.database
        .accept_operation_event(
            an_event(operation, Uuid::now_v7(), T0 + 5),
            T0 + 5,
            INTERVAL,
        )
        .await
        .expect("first accept");
    assert_eq!(
        binding_of(&test, operation).await.last_event_at,
        Some(T0 + 5)
    );

    test.database
        .accept_operation_event(
            an_event(operation, Uuid::now_v7(), T0 + 4),
            T0 + 6,
            INTERVAL,
        )
        .await
        .expect("stale accept");
    assert_eq!(
        binding_of(&test, operation).await.last_event_at,
        Some(T0 + 5),
        "a stale drop leaves the watermark"
    );

    test.database
        .accept_operation_event(
            an_event(operation, Uuid::now_v7(), T0 + 9),
            T0 + 9,
            INTERVAL,
        )
        .await
        .expect("newer accept");
    assert_eq!(
        binding_of(&test, operation).await.last_event_at,
        Some(T0 + 9),
        "an accepted event moves the watermark"
    );
}

/// The binding for one operation, through the existing pair-keyed read.
async fn binding_of(test: &TestDatabase, operation: Uuid) -> MessageBindingRecord {
    let chat_id: i64 =
        sqlx::query_scalar("select chat_id from telegram.message_bindings where operation_id = $1")
            .bind(operation)
            .fetch_one(test.pool())
            .await
            .expect("binding row");
    test.database
        .find_binding(operation, chat_id)
        .await
        .expect("find")
        .expect("binding exists")
}
