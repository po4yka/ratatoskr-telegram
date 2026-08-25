//! Inbox deduplication: the at-least-once event ledger the projection consumer writes through.
//!
//! Every consumed envelope carries an `event_id` that is globally unique per occurrence; the
//! insert IS the deduplication decision, exactly like update admission. These tests pin that a
//! redelivered event is reported as a duplicate without side effects anywhere else in the schema.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use sqlx::Row;
use telegram_persistence::RecordOutcome;
use telegram_persistence::test_support::TestDatabase;

async fn database() -> TestDatabase {
    TestDatabase::create()
        .await
        .expect("a disposable database must be creatable")
}

/// The same envelope id accepted twice reports Inserted once, then Duplicate forever.
#[test]
fn record_event_accepts_once_then_reports_duplicate() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let event_id = uuid::Uuid::now_v7();
        let first = db
            .record_event(event_id)
            .await
            .expect("the first record must succeed");
        assert_eq!(first, RecordOutcome::Inserted);

        let second = db
            .record_event(event_id)
            .await
            .expect("the redelivery must not error");
        assert_eq!(second, RecordOutcome::Duplicate);

        // A third arrival — at-least-once transport makes no promises — stays a duplicate.
        let third = db
            .record_event(event_id)
            .await
            .expect("the third delivery must not error");
        assert_eq!(third, RecordOutcome::Duplicate);

        // Exactly one row of evidence exists.
        let rows: i64 =
            sqlx::query("select count(*)::bigint as n from telegram.inbox where event_id = $1")
                .bind(event_id)
                .fetch_one(db.pool())
                .await
                .expect("count")
                .get("n");
        assert_eq!(rows, 1);

        test.cleanup().await.expect("cleanup");
    });
}

/// Recording an event touches nothing outside the inbox: no updates row, no binding, no job.
///
/// The inbox is evidence about EVENT consumption only; a shared-table accident here would couple
/// event deduplication to domain state that settles on its own facts.
#[test]
fn recording_an_event_leaves_the_rest_of_the_schema_untouched() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        db.record_event(uuid::Uuid::now_v7())
            .await
            .expect("the record");

        let updates: i64 = sqlx::query("select count(*)::bigint as n from telegram.updates")
            .fetch_one(db.pool())
            .await
            .expect("count")
            .get("n");
        let bindings: i64 =
            sqlx::query("select count(*)::bigint as n from telegram.message_bindings")
                .fetch_one(db.pool())
                .await
                .expect("count")
                .get("n");
        let jobs: i64 = sqlx::query("select count(*)::bigint as n from telegram.outbound_jobs")
            .fetch_one(db.pool())
            .await
            .expect("count")
            .get("n");

        assert_eq!((updates, bindings, jobs), (0, 0, 0));

        test.cleanup().await.expect("cleanup");
    });
}
