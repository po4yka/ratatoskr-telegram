//! The outbound delivery tables: operation message bindings, the durable Bot API job queue, and
//! the event inbox. Each test runs against its own disposable database.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use sqlx::Row;
use telegram_persistence::outbound_jobs::{DeliveryOutcome, NewOutboundJob, OutboundJobKind};
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

/// `(data_type, is_nullable)` of one column of one `telegram` table.
async fn column_shape(
    db: &telegram_persistence::Database,
    table: &str,
    column: &str,
) -> (String, String) {
    let row = sqlx::query(
        "select data_type, is_nullable
         from information_schema.columns
         where table_schema = 'telegram'
           and table_name = $1
           and column_name = $2",
    )
    .bind(table)
    .bind(column)
    .fetch_one(db.pool())
    .await
    .expect("the catalog read");
    (row.get("data_type"), row.get("is_nullable"))
}

/// Asserts one column carries exactly the expected type and nullability.
async fn expect_column(
    db: &telegram_persistence::Database,
    table: &str,
    column: &str,
    data_type: &str,
    nullable: &str,
) {
    let actual = column_shape(db, table, column).await;
    assert_eq!(
        actual,
        (data_type.to_owned(), nullable.to_owned()),
        "unexpected shape of {table}.{column}"
    );
}

/// The primary-key column names of one `telegram` table, in key order.
async fn primary_key(db: &telegram_persistence::Database, table: &str) -> Vec<String> {
    let rows = sqlx::query(
        "select a.attname as column_name
         from pg_index i
         join pg_class t on t.oid = i.indrelid
         join pg_attribute a on a.attrelid = t.oid and a.attnum = any(i.indkey)
         where t.relnamespace = 'telegram'::regnamespace
           and t.relname = $1
           and i.indisprimary
         order by a.attnum",
    )
    .bind(table)
    .fetch_all(db.pool())
    .await
    .expect("catalog read");
    rows.iter().map(|row| row.get("column_name")).collect()
}

/// The three outbound tables exist on a fresh database with app-minted UUID keys, their closed
/// vocabularies, working defaults, and the indexes the claim query depends on.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one catalog walk over three tables reads better as one test than as three \
              half-tests sharing setup"
)]
fn message_bindings_outbound_jobs_and_inbox_exist_with_expected_shape() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;

        // Existence first: everything below is meaningless until the relations are there.
        // information_schema rather than to_regclass, whose text form drops the schema qualifier
        // whenever the schema happens to sit on the search path.
        for table in ["message_bindings", "outbound_jobs", "inbox"] {
            let present: i64 = sqlx::query_scalar(
                "select count(*) from information_schema.tables
                 where table_schema = 'telegram' and table_name = $1",
            )
            .bind(table)
            .fetch_one(test.pool())
            .await
            .expect("catalog read");
            assert_eq!(present, 1, "relation telegram.{table} must exist");
        }

        // App-minted keys: message_bindings and outbound_jobs carry a surrogate uuid id; the
        // inbox key IS the envelope event id. None of them may carry a database default — a
        // missing id must be an insert error rather than a silently wrong version.
        for (table, key) in [
            ("message_bindings", "id"),
            ("outbound_jobs", "id"),
            ("inbox", "event_id"),
        ] {
            assert_eq!(primary_key(&test.database, table).await, [key]);
            let row = sqlx::query(
                "select column_default
                 from information_schema.columns
                 where table_schema = 'telegram'
                   and table_name = $1
                   and column_name = $2",
            )
            .bind(table)
            .bind(key)
            .fetch_one(test.pool())
            .await
            .expect("catalog read");
            assert!(
                row.get::<Option<String>, _>("column_default").is_none(),
                "{table}.{key} must carry no database default"
            );
        }

        // message_bindings: one live binding per (operation_id, chat_id).
        expect_column(&test.database, "message_bindings", "bot_id", "bigint", "NO").await;
        expect_column(&test.database, "message_bindings", "operation_id", "uuid", "NO").await;
        expect_column(&test.database, "message_bindings", "chat_id", "bigint", "NO").await;
        // The Telegram message id is unknown until a send is acknowledged, and unknown again
        // after an unbind; both states are NULL by design.
        expect_column(&test.database, "message_bindings", "message_id", "bigint", "YES").await;
        expect_column(
            &test.database,
            "message_bindings",
            "last_rendered_revision",
            "bigint",
            "NO",
        )
        .await;
        expect_column(
            &test.database,
            "message_bindings",
            "last_rendered_at",
            "timestamp with time zone",
            "YES",
        )
        .await;
        expect_column(&test.database, "message_bindings", "terminal", "boolean", "NO").await;
        expect_column(
            &test.database,
            "message_bindings",
            "created_at",
            "timestamp with time zone",
            "NO",
        )
        .await;
        expect_column(
            &test.database,
            "message_bindings",
            "updated_at",
            "timestamp with time zone",
            "NO",
        )
        .await;

        // outbound_jobs: the durable queue row.
        expect_column(&test.database, "outbound_jobs", "bot_id", "bigint", "NO").await;
        expect_column(&test.database, "outbound_jobs", "chat_id", "bigint", "NO").await;
        expect_column(&test.database, "outbound_jobs", "kind", "text", "NO").await;
        expect_column(&test.database, "outbound_jobs", "body", "text", "NO").await;
        expect_column(&test.database, "outbound_jobs", "content_hash", "text", "NO").await;
        expect_column(&test.database, "outbound_jobs", "operation_id", "uuid", "YES").await;
        expect_column(&test.database, "outbound_jobs", "revision", "bigint", "YES").await;
        expect_column(&test.database, "outbound_jobs", "correlation_id", "text", "YES").await;
        expect_column(&test.database, "outbound_jobs", "state", "text", "NO").await;
        expect_column(&test.database, "outbound_jobs", "attempts", "integer", "NO").await;
        expect_column(
            &test.database,
            "outbound_jobs",
            "next_attempt_at",
            "timestamp with time zone",
            "NO",
        )
        .await;
        expect_column(
            &test.database,
            "outbound_jobs",
            "lease_expires_at",
            "timestamp with time zone",
            "YES",
        )
        .await;
        expect_column(&test.database, "outbound_jobs", "last_error_class", "text", "YES").await;

        // inbox: bare event-id deduplication evidence.
        expect_column(&test.database, "inbox", "event_id", "uuid", "NO").await;
        expect_column(
            &test.database,
            "inbox",
            "seen_at",
            "timestamp with time zone",
            "NO",
        )
        .await;

        // A binding boots with revision 0, not terminal, timestamps stamped, no message yet.
        let operation = uuid::Uuid::now_v7();
        sqlx::query(
            "insert into telegram.message_bindings (id, bot_id, operation_id, chat_id)
             values ($1, 700100200, $2, 900700600)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(operation)
        .execute(test.pool())
        .await
        .expect("the bare binding insert");
        let row = sqlx::query(
            "select last_rendered_revision, terminal, message_id is null as unbound,
                    created_at is not null as created_stamped,
                    updated_at is not null as updated_stamped
             from telegram.message_bindings
             where operation_id = $1 and chat_id = 900700600",
        )
        .bind(operation)
        .fetch_one(test.pool())
        .await
        .expect("the binding row");
        assert_eq!(row.get::<i64, _>("last_rendered_revision"), 0);
        assert!(!row.get::<bool, _>("terminal"));
        assert!(row.get::<bool, _>("unbound"));
        assert!(row.get::<bool, _>("created_stamped"));
        assert!(row.get::<bool, _>("updated_stamped"));

        // One live binding per (operation_id, chat_id): the unique constraint refuses a second.
        let duplicate = sqlx::query(
            "insert into telegram.message_bindings (id, bot_id, operation_id, chat_id)
             values ($1, 700100200, $2, 900700600)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(operation)
        .execute(test.pool())
        .await;
        assert!(
            duplicate.is_err(),
            "a second binding for one (operation, chat) must violate the unique constraint"
        );

        // A job boots ready with zero attempts, an immediate next attempt, and no lease.
        let job_id = uuid::Uuid::now_v7();
        sqlx::query(
            "insert into telegram.outbound_jobs (id, bot_id, chat_id, kind, body, content_hash)
             values ($1, 700100200, 900700600, 'send_message', 'text', 'hash')",
        )
        .bind(job_id)
        .execute(test.pool())
        .await
        .expect("the bare job insert");
        let row = sqlx::query(
            "select state, attempts, next_attempt_at is not null as scheduled,
                    lease_expires_at is null as unleased, last_error_class is null as clean
             from telegram.outbound_jobs where id = $1",
        )
        .bind(job_id)
        .fetch_one(test.pool())
        .await
        .expect("the job row");
        assert_eq!(row.get::<&str, _>("state"), "ready");
        assert_eq!(row.get::<i32, _>("attempts"), 0);
        assert!(row.get::<bool, _>("scheduled"));
        assert!(row.get::<bool, _>("unleased"));
        assert!(row.get::<bool, _>("clean"));

        // Both job vocabularies are closed by CHECK constraints.
        let bogus_kind = sqlx::query(
            "insert into telegram.outbound_jobs (id, bot_id, chat_id, kind, body, content_hash)
             values ($1, 700100200, 900700601, 'delete_message', 'text', 'hash')",
        )
        .bind(uuid::Uuid::now_v7())
        .execute(test.pool())
        .await;
        assert!(
            bogus_kind.is_err(),
            "an unknown job kind must violate the check constraint"
        );
        let bogus_state = sqlx::query(
            "insert into telegram.outbound_jobs (id, bot_id, chat_id, kind, body, content_hash, state)
             values ($1, 700100200, 900700601, 'send_message', 'text', 'hash', 'queued')",
        )
        .bind(uuid::Uuid::now_v7())
        .execute(test.pool())
        .await;
        assert!(
            bogus_state.is_err(),
            "an unknown job state must violate the check constraint"
        );

        // An inbox row stamps its arrival time.
        sqlx::query("insert into telegram.inbox (event_id) values ($1)")
            .bind(uuid::Uuid::now_v7())
            .execute(test.pool())
            .await
            .expect("the bare inbox insert");
        let row = sqlx::query("select seen_at is not null as stamped from telegram.inbox limit 1")
            .fetch_one(test.pool())
            .await
            .expect("the inbox row");
        assert!(row.get::<bool, _>("stamped"));

        // The claim query leans on three indexes; their absence would only surface as a slow
        // queue under load, so pin them here.
        let indexes: Vec<String> = sqlx::query(
            "select indexname from pg_indexes
             where schemaname = 'telegram' and tablename = 'outbound_jobs'",
        )
        .fetch_all(test.pool())
        .await
        .expect("catalog read")
        .iter()
        .map(|row| row.get::<String, _>("indexname"))
        .collect();
        for expected in [
            "outbound_jobs_due_idx",
            "outbound_jobs_chat_idx",
            "outbound_jobs_sending_idx",
        ] {
            assert!(
                indexes.iter().any(|name| name == expected),
                "index {expected} must exist; found {indexes:?}"
            );
        }

        test.cleanup().await.expect("cleanup");
    });
}

/// The bot id every job fixture shares.
const BOT: i64 = 700_100_200;
/// Chat A of the FIFO test: deliberately the LOWEST id, so a claim that ever preferred A while a
/// fresh lease holds it would be caught rather than masked by chat-id ordering.
const CHAT_A: i64 = 900_700_610;
/// Chat B of the FIFO test.
const CHAT_B: i64 = 900_700_620;
/// Chat C of the FIFO test.
const CHAT_C: i64 = 900_700_630;

/// The canonical sha256 digest of the body `"abc"`, used as the caller-computed content hash.
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

/// A send-job fixture with the given rendered body and its caller-computed hash.
fn send_job(chat_id: i64, body: &str, content_hash: &str) -> NewOutboundJob {
    NewOutboundJob {
        bot_id: BOT,
        chat_id,
        kind: OutboundJobKind::SendMessage,
        body: body.to_owned(),
        content_hash: content_hash.to_owned(),
        operation_id: None,
        revision: None,
        correlation_id: None,
        next_attempt_at: None,
    }
}

/// An edit-job fixture bound to `operation` at `revision`.
fn edit_job(chat_id: i64, operation: uuid::Uuid, revision: i64) -> NewOutboundJob {
    NewOutboundJob {
        bot_id: BOT,
        chat_id,
        kind: OutboundJobKind::EditMessageText,
        body: format!("render {revision}"),
        content_hash: format!("hash-{revision}"),
        operation_id: Some(operation),
        revision: Some(revision),
        correlation_id: None,
        next_attempt_at: None,
    }
}

/// The raw `state` string of one job row.
async fn job_state(db: &telegram_persistence::Database, id: uuid::Uuid) -> String {
    sqlx::query_scalar("select state from telegram.outbound_jobs where id = $1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .expect("the job row")
}

/// Whole seconds since the Unix epoch for one job's `next_attempt_at`.
async fn job_next_attempt_epoch(db: &telegram_persistence::Database, id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "select extract(epoch from next_attempt_at)::bigint from telegram.outbound_jobs
         where id = $1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .expect("the job row")
}

/// An enqueued send job is ready immediately, stores the caller's content hash verbatim, and is
/// scheduled for an immediate attempt; claiming it returns the full payload.
#[test]
fn enqueue_marks_ready_and_persists_payload_hash() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let operation = uuid::Uuid::now_v7();
        let id = db
            .enqueue_outbound_job(
                &NewOutboundJob {
                    bot_id: BOT,
                    chat_id: CHAT_A,
                    kind: OutboundJobKind::SendMessage,
                    body: "abc".to_owned(),
                    content_hash: ABC_SHA256.to_owned(),
                    operation_id: Some(operation),
                    revision: Some(7),
                    correlation_id: Some("correlation".to_owned()),
                    next_attempt_at: None,
                },
                T0,
            )
            .await
            .expect("the enqueue");

        // Ready and due exactly at the caller's clock: with no explicit schedule the job is
        // stamped at the `now` the enqueue was handed, so every scheduling comparison stays on
        // one clock and this assertion is exact rather than a tolerance window.
        assert_eq!(job_state(db, id).await, "ready");
        let scheduled = job_next_attempt_epoch(db, id).await;
        assert_eq!(scheduled, T0, "no schedule must mean due at the handed now");

        // The stored hash is exactly what the caller computed — persistence does not hash.
        let stored: String =
            sqlx::query_scalar("select content_hash from telegram.outbound_jobs where id = $1")
                .bind(id)
                .fetch_one(db.pool())
                .await
                .expect("the job row");
        assert_eq!(stored, ABC_SHA256);

        // Claiming at the handed clock returns the whole payload for the Bot API call.
        let claimed = db
            .claim_due_outbound_job(T0, 30)
            .await
            .expect("the claim")
            .expect("the enqueued job to be claimable");
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.bot_id, BOT);
        assert_eq!(claimed.chat_id, CHAT_A);
        assert_eq!(claimed.kind, OutboundJobKind::SendMessage);
        assert_eq!(claimed.body, "abc");
        assert_eq!(claimed.content_hash, ABC_SHA256);
        assert_eq!(claimed.operation_id, Some(operation));
        assert_eq!(claimed.revision, Some(7));
        assert_eq!(claimed.correlation_id.as_deref(), Some("correlation"));
        assert_eq!(claimed.attempts, 1, "attempts increment at claim time");

        test.cleanup().await.expect("cleanup");
    });
}

/// Claims yield strict per-chat FIFO heads: A's earliest first, then other chats' heads while A
/// is in flight, and A's second job only after A's first settles. No ordering promise across
/// chats is made — but one in-flight job per chat IS enforced.
#[test]
fn claim_returns_strict_per_chat_fifo_heads_only() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        // Interleaved enqueues; UUIDv7 ids ascend with insertion order.
        let a1 = db
            .enqueue_outbound_job(&send_job(CHAT_A, "a1", "h-a1"), T0)
            .await
            .expect("enqueue a1");
        let b1 = db
            .enqueue_outbound_job(&send_job(CHAT_B, "b1", "h-b1"), T0)
            .await
            .expect("enqueue b1");
        let c1 = db
            .enqueue_outbound_job(&send_job(CHAT_C, "c1", "h-c1"), T0)
            .await
            .expect("enqueue c1");
        let a2 = db
            .enqueue_outbound_job(&send_job(CHAT_A, "a2", "h-a2"), T0)
            .await
            .expect("enqueue a2");

        // First claim: chat A's EARLIEST job, nothing else.
        let first = db
            .claim_due_outbound_job(T0, 30)
            .await
            .expect("the first claim")
            .expect("chat A's head");
        assert_eq!(first.id, a1, "the first claim must return A's earliest job");

        // While A1 is in flight on a fresh lease, claims skip chat A entirely: B's head, then
        // C's head — never A2.
        let second = db
            .claim_due_outbound_job(T0 + 1, 30)
            .await
            .expect("the second claim")
            .expect("chat B's head");
        assert_eq!(second.id, b1, "an in-flight A must not block other chats");

        let third = db
            .claim_due_outbound_job(T0 + 2, 30)
            .await
            .expect("the third claim")
            .expect("chat C's head");
        assert_eq!(third.id, c1);

        // Every chat now has either an in-flight job or nothing eligible: no fourth claim.
        let fourth = db
            .claim_due_outbound_job(T0 + 3, 30)
            .await
            .expect("the fourth claim");
        assert!(
            fourth.is_none(),
            "A's second job must wait while A's first is in flight"
        );

        // Settling A1 terminally frees the chat: A2 becomes the next head.
        db.settle_outbound_job(a1, T0 + 4, 3, &DeliveryOutcome::Sent)
            .await
            .expect("the settlement of a1");
        let fifth = db
            .claim_due_outbound_job(T0 + 5, 30)
            .await
            .expect("the fifth claim")
            .expect("chat A's second job after the first settled");
        assert_eq!(fifth.id, a2, "A must resume with its own FIFO tail");

        test.cleanup().await.expect("cleanup");
    });
}

/// Enqueueing a newer edit supersedes older still-waiting edits of the same binding, and never
/// touches a job already in flight.
#[test]
fn supersede_marks_stale_ready_jobs_without_touching_in_flight() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let operation = uuid::Uuid::now_v7();

        let r4 = db
            .enqueue_outbound_job(&edit_job(CHAT_A, operation, 4), T0)
            .await
            .expect("enqueue rev4");
        let r5 = db
            .enqueue_outbound_job(&edit_job(CHAT_A, operation, 5), T0)
            .await
            .expect("enqueue rev5");

        assert_eq!(
            job_state(db, r4).await,
            "superseded",
            "the older ready edit must be superseded by the newer enqueue"
        );
        assert_eq!(job_state(db, r5).await, "ready");

        // Take revision 6 in flight.
        let r6 = db
            .enqueue_outbound_job(&edit_job(CHAT_A, operation, 6), T0)
            .await
            .expect("enqueue rev6");
        let claimed = db
            .claim_due_outbound_job(T0, 30)
            .await
            .expect("the claim")
            .expect("revision 6 to be claimable");
        assert_eq!(claimed.id, r6);
        assert_eq!(job_state(db, r6).await, "sending");

        // Revision 7 arrives while 6 is in flight: the sweep must leave 6 alone.
        let r7 = db
            .enqueue_outbound_job(&edit_job(CHAT_A, operation, 7), T0)
            .await
            .expect("enqueue rev7");
        assert_eq!(
            job_state(db, r6).await,
            "sending",
            "an in-flight job must never be superseded from the queue"
        );
        assert_eq!(job_state(db, r7).await, "ready");

        test.cleanup().await.expect("cleanup");
    });
}

/// A sending row whose lease has expired is reclaimed by a later claim; a fresh lease is not.
#[test]
fn lease_expiry_reclaims_sending_rows_after_ttl() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let id = db
            .enqueue_outbound_job(&send_job(CHAT_A, "body", "h"), T0)
            .await
            .expect("the enqueue");

        let claimed = db
            .claim_due_outbound_job(T0, 30)
            .await
            .expect("the first claim")
            .expect("the job");
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.attempts, 1);

        // One second before the lease runs out: nothing else is claimable.
        let fresh = db
            .claim_due_outbound_job(T0 + 29, 30)
            .await
            .expect("the mid-lease claim");
        assert!(fresh.is_none(), "a fresh lease must not be reclaimed early");

        // Past the lease: the same job comes back, attempts counted across the crash boundary.
        let reclaimed = db
            .claim_due_outbound_job(T0 + 31, 30)
            .await
            .expect("the reclaiming claim")
            .expect("the expired lease to be reclaimed");
        assert_eq!(reclaimed.id, id);
        assert_eq!(
            reclaimed.attempts, 2,
            "attempts must count every claim, including reclaims"
        );

        test.cleanup().await.expect("cleanup");
    });
}

/// A transient outcome reschedules into `retry_wait` at now + delay, keeping the attempt count
/// the claim produced.
#[test]
fn retry_reschedules_with_backoff_state() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let id = db
            .enqueue_outbound_job(&send_job(CHAT_A, "body", "h"), T0)
            .await
            .expect("the enqueue");
        let claimed = db
            .claim_due_outbound_job(T0, 30)
            .await
            .expect("the claim")
            .expect("the job");
        assert_eq!(claimed.attempts, 1);

        db.settle_outbound_job(
            id,
            T0 + 5,
            3,
            &DeliveryOutcome::RetryWithBackoff { delay_secs: 120 },
        )
        .await
        .expect("the retry settlement");

        assert_eq!(job_state(db, id).await, "retry_wait");
        assert_eq!(
            job_next_attempt_epoch(db, id).await,
            T0 + 5 + 120,
            "the retry must reschedule at settle time plus the backoff delay"
        );
        let lease_cleared: bool = sqlx::query_scalar(
            "select lease_expires_at is null from telegram.outbound_jobs where id = $1",
        )
        .bind(id)
        .fetch_one(db.pool())
        .await
        .expect("the job row");
        assert!(lease_cleared, "a waiting job holds no lease");

        test.cleanup().await.expect("cleanup");
    });
}

/// Transient retries dead-letter once the attempt count reaches the configured bound.
#[test]
fn dead_letter_after_bound_attempts() {
    const MAX_ATTEMPTS: i32 = 2;

    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let id = db
            .enqueue_outbound_job(&send_job(CHAT_A, "body", "h"), T0)
            .await
            .expect("the enqueue");

        // Attempt one fails transiently inside the bound: retry_wait.
        db.claim_due_outbound_job(T0, 30)
            .await
            .expect("the first claim")
            .expect("the job");
        db.settle_outbound_job(
            id,
            T0 + 1,
            MAX_ATTEMPTS,
            &DeliveryOutcome::RetryWithBackoff { delay_secs: 10 },
        )
        .await
        .expect("the first retry settlement");
        assert_eq!(job_state(db, id).await, "retry_wait");

        // Attempt two lands ON the bound: the next transient outcome dead-letters instead of
        // scheduling an unbounded retry loop.
        db.claim_due_outbound_job(T0 + 11, 30)
            .await
            .expect("the second claim")
            .expect("the rescheduled job");
        db.settle_outbound_job(
            id,
            T0 + 12,
            MAX_ATTEMPTS,
            &DeliveryOutcome::RetryWithBackoff { delay_secs: 10 },
        )
        .await
        .expect("the exhausted settlement");

        let (state, error_class): (String, Option<String>) = sqlx::query_as(
            "select state, last_error_class from telegram.outbound_jobs where id = $1",
        )
        .bind(id)
        .fetch_one(db.pool())
        .await
        .expect("the job row");
        assert_eq!(state, "failed_permanent");
        assert_eq!(
            error_class.as_deref(),
            Some("transient"),
            "dead-lettering after exhaustion records the transient class"
        );

        test.cleanup().await.expect("cleanup");
    });
}

/// Terminal outcomes settle `sent` — both a real success and the message-not-modified no-op —
/// clear the lease, and show up in the queue-depth counts.
#[test]
fn terminal_success_settles_sent() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let sent_id = db
            .enqueue_outbound_job(&send_job(CHAT_A, "body", "h"), T0)
            .await
            .expect("the enqueue");
        db.claim_due_outbound_job(T0, 30)
            .await
            .expect("the claim")
            .expect("the job");
        db.settle_outbound_job(sent_id, T0 + 1, 3, &DeliveryOutcome::Sent)
            .await
            .expect("the sent settlement");
        assert_eq!(job_state(db, sent_id).await, "sent");

        let noop_id = db
            .enqueue_outbound_job(&edit_job(CHAT_B, uuid::Uuid::now_v7(), 9), T0)
            .await
            .expect("the enqueue");
        db.claim_due_outbound_job(T0 + 2, 30)
            .await
            .expect("the claim")
            .expect("the job");
        db.settle_outbound_job(noop_id, T0 + 3, 3, &DeliveryOutcome::NotModified)
            .await
            .expect("the not-modified settlement");
        assert_eq!(
            job_state(db, noop_id).await,
            "sent",
            "message-not-modified is a successful no-op"
        );

        let mut counts = db
            .count_outbound_jobs_by_state()
            .await
            .expect("the queue depth");
        counts.sort();
        assert_eq!(counts, vec![("sent".to_owned(), 2)]);

        test.cleanup().await.expect("cleanup");
    });
}
