//! Message-binding repository behavior: insert-if-absent creation, idempotent send
//! acknowledgments, monotonic render revisions, unbind, and the once-only terminal flag.
//!
//! These are the semantics the dispatcher's sender and projection consumer both lean on, so they
//! are pinned at the persistence boundary where they live rather than re-derived downstream.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use telegram_persistence::test_support::TestDatabase;

/// A connected pool over the disposable database, for raw assertions.
async fn database() -> TestDatabase {
    TestDatabase::create()
        .await
        .expect("a disposable database must be creatable")
}

/// A fixed synthetic instant: whole seconds since the Unix epoch, never read from a wall clock.
/// The repository takes every timestamp from its caller, so tests pin exact values.
const T0: i64 = 1_800_000_000;
/// A later fixed instant, half a minute after [`T0`].
const T1: i64 = 1_800_000_030;

/// A binding is created insert-if-absent and found again by its (operation, chat) pair; a fresh
/// binding has no message yet, revision 0, and is not terminal.
#[test]
fn binding_is_created_and_found_by_operation() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let bot_id = 700_100_200;
        let chat_id = 900_700_600;
        let operation = uuid::Uuid::now_v7();

        let created = db
            .ensure_operation_binding(bot_id, operation, chat_id)
            .await
            .expect("the create");
        assert_eq!(created.bot_id, bot_id);
        assert_eq!(created.operation_id, operation);
        assert_eq!(created.chat_id, chat_id);
        assert_eq!(created.message_id, None, "no send acknowledged yet");
        assert_eq!(created.last_rendered_revision, 0);
        assert_eq!(created.last_rendered_at, None);
        assert!(!created.terminal);

        let found = db
            .find_binding(operation, chat_id)
            .await
            .expect("the find")
            .expect("the binding to be present");
        assert_eq!(found, created);

        let missing = db.find_binding(operation, 999_999).await.expect("the find");
        assert_eq!(missing, None);

        test.cleanup().await.expect("cleanup");
    });
}

/// The send acknowledgment upserts the message id: re-acking the same (operation, chat) keeps
/// exactly one row and leaves the LATEST acknowledged message id in place.
#[test]
fn ensure_send_binding_is_idempotent_on_repeat_ack() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let bot_id = 700_100_200;
        let chat_id = 900_700_601;
        let operation = uuid::Uuid::now_v7();

        db.record_send_acknowledged(bot_id, operation, chat_id, 41, T0)
            .await
            .expect("the first ack");
        db.record_send_acknowledged(bot_id, operation, chat_id, 42, T1)
            .await
            .expect("the second ack");

        let rows: i64 = sqlx::query_scalar(
            "select count(*) from telegram.message_bindings
             where operation_id = $1 and chat_id = $2",
        )
        .bind(operation)
        .bind(chat_id)
        .fetch_one(db.pool())
        .await
        .expect("the count query");
        assert_eq!(rows, 1, "a repeat ack must keep exactly one binding row");

        let found = db
            .find_binding(operation, chat_id)
            .await
            .expect("the find")
            .expect("the binding to be present");
        assert_eq!(
            found.message_id,
            Some(42),
            "the latest acknowledged message id must win"
        );

        test.cleanup().await.expect("cleanup");
    });
}

/// Revisions advance monotonically: a newer revision applies and stamps `last_rendered_at`; an
/// older one is refused as stale and changes nothing.
#[test]
fn last_rendered_revision_advances_monotonically() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let chat_id = 900_700_602;
        let operation = uuid::Uuid::now_v7();
        db.ensure_operation_binding(700_100_200, operation, chat_id)
            .await
            .expect("the create");

        let applied = db
            .advance_render(operation, chat_id, 5, T0)
            .await
            .expect("the advance to 5");
        assert!(applied, "advancing to a newer revision must apply");

        let stale = db
            .advance_render(operation, chat_id, 3, T1)
            .await
            .expect("the advance to 3");
        assert!(!stale, "an older revision must not apply");

        let found = db
            .find_binding(operation, chat_id)
            .await
            .expect("the find")
            .expect("the binding to be present");
        assert_eq!(found.last_rendered_revision, 5, "revision must stay at 5");
        assert_eq!(
            found.last_rendered_at,
            Some(T0),
            "last_rendered_at must stamp on the successful advance"
        );

        test.cleanup().await.expect("cleanup");
    });
}

/// Unbinding clears the message id while keeping the binding row: after a permanent edit failure
/// the next revision sends a fresh message and rebinds instead of killing all rendering.
#[test]
fn unbind_clears_the_message_but_keeps_the_binding() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let chat_id = 900_700_603;
        let operation = uuid::Uuid::now_v7();
        db.record_send_acknowledged(700_100_200, operation, chat_id, 7, T0)
            .await
            .expect("the ack");

        db.unbind_message(operation, chat_id, T1)
            .await
            .expect("the unbind");

        let found = db
            .find_binding(operation, chat_id)
            .await
            .expect("the find")
            .expect("the binding must survive an unbind");
        assert_eq!(found.message_id, None, "unbind must clear the message id");

        test.cleanup().await.expect("cleanup");
    });
}

/// The terminal flag applies exactly once: the first call wins, later calls report
/// already-terminal without changing anything — second terminals are dropped downstream on this
/// exact answer.
#[test]
fn mark_terminal_applies_once_then_reports_already_terminal() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let chat_id = 900_700_604;
        let operation = uuid::Uuid::now_v7();
        db.ensure_operation_binding(700_100_200, operation, chat_id)
            .await
            .expect("the create");

        let first = db
            .mark_terminal(operation, chat_id, T0)
            .await
            .expect("the first mark");
        assert!(first, "the first terminal transition must apply");

        let second = db
            .mark_terminal(operation, chat_id, T1)
            .await
            .expect("the second mark");
        assert!(
            !second,
            "a second terminal transition must report already-terminal"
        );

        let found = db
            .find_binding(operation, chat_id)
            .await
            .expect("the find")
            .expect("the binding to be present");
        assert!(found.terminal);

        test.cleanup().await.expect("cleanup");
    });
}

// ---------------------------------------------------------------------------
// outbound jobs repository
// ---------------------------------------------------------------------------
