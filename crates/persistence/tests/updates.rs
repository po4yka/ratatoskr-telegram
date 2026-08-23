//! The `telegram.updates` table: the composite key, the insert-or-ignore decision, and the typed
//! state transitions. Each test runs against its own disposable database.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use sqlx::Row;
use telegram_persistence::test_support::TestDatabase;
use telegram_persistence::updates::{AdmittedUpdate, RecordOutcome, UpdateState};

/// A connected pool over the disposable database, for raw assertions.
async fn database() -> TestDatabase {
    TestDatabase::create()
        .await
        .expect("a disposable database must be creatable")
}

fn admitted(bot_id: i64, update_id: i64) -> AdmittedUpdate {
    AdmittedUpdate {
        bot_id,
        update_id,
        kind: "message".to_owned(),
        payload: format!(r#"{{"update_id":{update_id}}}"#),
    }
}

async fn row_count(db: &telegram_persistence::Database) -> i64 {
    sqlx::query("select count(*)::bigint as n from telegram.updates")
        .fetch_one(db.pool())
        .await
        .expect("the count query")
        .get("n")
}

/// The table exists on a fresh database with the composite key and the closed state vocabulary.
#[test]
fn the_schema_defines_the_updates_table() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        // The composite key: both columns participate, in this order.
        let key = sqlx::query(
            "select a.attname as column_name
             from pg_index i
             join pg_class t on t.oid = i.indrelid
             join pg_attribute a on a.attrelid = t.oid and a.attnum = any(i.indkey)
             where t.relnamespace = 'telegram'::regnamespace
               and t.relname = 'updates'
               and i.indisprimary
             order by a.attnum",
        )
        .fetch_all(test.pool())
        .await
        .expect("catalog read");
        let names: Vec<String> = key.iter().map(|row| row.get("column_name")).collect();
        assert_eq!(names, ["bot_id", "update_id"]);

        // The state vocabulary is closed by a CHECK.
        let bogus = sqlx::query(
            "insert into telegram.updates (bot_id, update_id, kind, state)
             values (1, 2, 'message', 'bogus')",
        )
        .execute(test.pool())
        .await;
        assert!(
            bogus.is_err(),
            "an unknown state must violate the check constraint"
        );

        test.cleanup().await.expect("cleanup");
    });
}

/// Insert-or-ignore decides acceptance: first delivery inserted, redelivery duplicate, one row.
#[test]
fn first_insert_is_inserted_and_the_redelivery_is_a_duplicate() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        assert!(matches!(
            db.record_update(&admitted(700_100_200, 42)).await,
            Ok(RecordOutcome::Inserted),
        ));
        assert!(matches!(
            db.record_update(&admitted(700_100_200, 42)).await,
            Ok(RecordOutcome::Duplicate),
        ));
        assert_eq!(row_count(db).await, 1);
        test.cleanup().await.expect("cleanup");
    });
}

/// The key is bot-scoped: two bots may carry the same update id independently.
#[test]
fn the_same_update_id_under_another_bot_is_not_a_duplicate() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        assert!(matches!(
            db.record_update(&admitted(700_100_200, 42)).await,
            Ok(RecordOutcome::Inserted),
        ));
        assert!(matches!(
            db.record_update(&admitted(700_100_201, 42)).await,
            Ok(RecordOutcome::Inserted),
        ));
        assert_eq!(row_count(db).await, 2);
        test.cleanup().await.expect("cleanup");
    });
}

/// An admitted row starts accepted with kind and receipt time recorded; settlement moves it to a
/// terminal state with a settle time.
#[test]
fn settlement_moves_an_admitted_row_to_its_terminal_state() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        db.record_update(&AdmittedUpdate {
            bot_id: 700_100_200,
            update_id: 7,
            kind: "callback_query".to_owned(),
            payload: r#"{"update_id":7}"#.to_owned(),
        })
        .await
        .expect("the insert");

        let before = sqlx::query(
            "select kind, state, received_at is not null as stamped, settled_at is null as open
             from telegram.updates where bot_id = 700100200 and update_id = 7",
        )
        .fetch_one(db.pool())
        .await
        .expect("the row");
        assert_eq!(before.get::<&str, _>("kind"), "callback_query");
        assert_eq!(before.get::<&str, _>("state"), "accepted");
        assert!(before.get::<bool, _>("stamped"));
        assert!(before.get::<bool, _>("open"));

        db.settle_update(700_100_200, 7, UpdateState::Processed)
            .await
            .expect("the settlement");

        let after = sqlx::query(
            "select state = 'processed' as settled_processed, settled_at is not null as settled
             from telegram.updates where bot_id = 700100200 and update_id = 7",
        )
        .fetch_one(db.pool())
        .await
        .expect("the row");
        assert!(after.get::<bool, _>("settled_processed"));
        assert!(after.get::<bool, _>("settled"));
        test.cleanup().await.expect("cleanup");
    });
}

/// Terminal settlement keeps the dedupe evidence but removes the processable payload.
#[test]
fn terminal_settlement_removes_the_processable_payload() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        db.record_update(&admitted(700_100_200, 8))
            .await
            .expect("the insert");
        db.settle_update(700_100_200, 8, UpdateState::Processed)
            .await
            .expect("the terminal settlement");

        let row = sqlx::query(
            "select bot_id, update_id, kind, state, payload is null as payload_removed
             from telegram.updates where bot_id = 700100200 and update_id = 8",
        )
        .fetch_one(db.pool())
        .await
        .expect("the settled row");
        let bot_id = row.get::<i64, _>("bot_id");
        let update_id = row.get::<i64, _>("update_id");
        let kind = row.get::<&str, _>("kind").to_owned();
        let state = row.get::<&str, _>("state").to_owned();
        let payload_removed = row.get::<bool, _>("payload_removed");
        test.cleanup().await.expect("cleanup");

        assert_eq!((bot_id, update_id), (700_100_200, 8));
        assert_eq!(kind, "message");
        assert_eq!(state, "processed");
        assert!(payload_removed, "terminal payload must be removed");
    });
}

/// Settling an update that was never admitted fails and writes nothing.
#[test]
fn settling_an_unknown_pair_fails_writing_nothing() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let outcome = db
            .settle_update(700_100_999, 99, UpdateState::Unsupported)
            .await;
        assert!(outcome.is_err(), "an unknown pair must fail");
        assert_eq!(row_count(db).await, 0);
        test.cleanup().await.expect("cleanup");
    });
}
