//! Accepted-capture projection recovery, concurrency, and bounded retry behavior.

use super::*;

struct ProjectionCounts {
    bindings: i64,
    intents: i64,
    outbound: i64,
}

fn concurrent_capture_context(base_url: &str) -> (CaptureContext, tempfile::TempDir) {
    let client = platform_api::Client::new(&platform_api_url(base_url), Duration::from_secs(5))
        .expect("the platform client builds");
    let issuer = platform_api::assertion::AssertionIssuer::from_seed(&SEED, AUDIENCE)
        .expect("the issuer builds");
    let sessions = Arc::new(platform_api::session::SessionSource::new(
        client,
        issuer,
        Box::new(platform_api::session::SystemClock),
    ));
    let bot_api = bot_api::Client::new(
        &SecretString::new("synthetic-bot-token".into()),
        &platform_api_url(base_url),
        Duration::from_secs(5),
    )
    .expect("the synthetic Bot API client builds");
    let blob_root = tempfile::tempdir().expect("blob root");
    let blobs =
        ratatoskr_telegram_blob_store::BlobStore::open(blob_root.path()).expect("blob store opens");
    (
        CaptureContext::new(sessions, bot_api, blobs, 1024),
        blob_root,
    )
}

async fn projection_counts(database: &TestDatabase, operation: uuid::Uuid) -> ProjectionCounts {
    let bindings = sqlx::query_scalar(
        "select count(*) from telegram.message_bindings where operation_id = $1",
    )
    .bind(operation)
    .fetch_one(database.pool())
    .await
    .expect("binding count");
    let intents = sqlx::query_scalar(
        "select count(*) from telegram.interaction_tokens
         where surface = 'deep_link' and action = 'operation_status' and operation_id = $1",
    )
    .bind(operation)
    .fetch_one(database.pool())
    .await
    .expect("operation intent count");
    let outbound = sqlx::query_scalar(
        "select count(*) from telegram.outbound_jobs
         where kind = 'send_message' and operation_id = $1",
    )
    .bind(operation)
    .fetch_one(database.pool())
    .await
    .expect("acknowledgement count");
    ProjectionCounts {
        bindings,
        intents,
        outbound,
    }
}

/// Platform acceptance followed by an acknowledgement-insert failure must not leave either of the
/// earlier local projection rows behind. The trigger is scoped to operation acknowledgement sends
/// in this test's disposable database and is removed before that database is dropped.
#[tokio::test]
async fn accepted_capture_projection_is_all_or_nothing() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let (fixture, mut receiver, context) = Fixture::prepare(&base_url).await;
    sqlx::query(
        "create function telegram.reject_capture_ack_for_test() returns trigger
         language plpgsql as $$
         begin
             raise exception 'synthetic acknowledgement insertion failure';
         end
         $$",
    )
    .execute(fixture.database.pool())
    .await
    .expect("the scoped fault function is installed");
    sqlx::query(
        "create trigger reject_capture_ack_for_test
         before insert on telegram.outbound_jobs
         for each row
         when (new.kind = 'send_message' and new.operation_id is not null)
         execute function telegram.reject_capture_ack_for_test()",
    )
    .execute(fixture.database.pool())
    .await
    .expect("the scoped acknowledgement fault is installed");

    fixture
        .deliver(message_update(9_051, "https://example.test/article"))
        .await;
    let update = receiver.recv().await.expect("the admitted update");
    intake::process_one(&fixture.database.database, &update, Some(&context)).await;

    let operation: uuid::Uuid = OPERATION_ID.parse().expect("synthetic uuid");
    let capture_calls = state.capture_calls.load(Ordering::SeqCst);
    let bindings = binding_count(&fixture, operation).await;
    let intents: i64 = sqlx::query_scalar(
        "select count(*) from telegram.interaction_tokens
         where surface = 'deep_link' and action = 'operation_status' and operation_id = $1",
    )
    .bind(operation)
    .fetch_one(fixture.database.pool())
    .await
    .expect("operation intent count");
    let outbound = outbound_job_count(&fixture).await;

    sqlx::query("drop trigger reject_capture_ack_for_test on telegram.outbound_jobs")
        .execute(fixture.database.pool())
        .await
        .expect("the scoped acknowledgement fault is removed");
    sqlx::query("drop function telegram.reject_capture_ack_for_test()")
        .execute(fixture.database.pool())
        .await
        .expect("the scoped fault function is removed");
    let Fixture {
        database,
        app,
        blob_root,
    } = fixture;
    drop(context);
    drop(app);
    drop(blob_root);
    database
        .cleanup()
        .await
        .expect("the disposable database is dropped before assertions");

    assert_eq!(capture_calls, 1, "Platform accepted exactly one capture");
    assert_eq!(
        bindings, 0,
        "accepted projection must leave no partial binding"
    );
    assert_eq!(
        intents, 0,
        "accepted projection must leave no partial operation intent"
    );
    assert_eq!(
        outbound, 0,
        "the rejected acknowledgement must leave no outbound job"
    );
}

/// A one-shot local fault after Platform acceptance must retain the update long enough to replay
/// the same external command identity and converge on one complete local projection.
#[tokio::test]
async fn accepted_capture_retries_projection_after_storage_failure() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;
    sqlx::query("create sequence telegram.capture_projection_fault_once_for_test")
        .execute(fixture.database.pool())
        .await
        .expect("the one-shot fault sequence is installed");
    sqlx::query(
        "create function telegram.reject_first_capture_ack_for_test() returns trigger
         language plpgsql as $$
         begin
             if nextval('telegram.capture_projection_fault_once_for_test') = 1 then
                 raise exception 'synthetic one-shot acknowledgement insertion failure';
             end if;
             return new;
         end
         $$",
    )
    .execute(fixture.database.pool())
    .await
    .expect("the one-shot fault function is installed");
    sqlx::query(
        "create trigger reject_first_capture_ack_for_test
         before insert on telegram.outbound_jobs
         for each row
         when (new.kind = 'send_message' and new.operation_id is not null)
         execute function telegram.reject_first_capture_ack_for_test()",
    )
    .execute(fixture.database.pool())
    .await
    .expect("the one-shot acknowledgement fault is installed");

    fixture
        .deliver(message_update(9_052, "https://example.test/article"))
        .await;
    let final_state = fixture.settled_state(9_052).await;

    let operation: uuid::Uuid = OPERATION_ID.parse().expect("synthetic uuid");
    let payload_present: bool = sqlx::query_scalar(
        "select payload is not null from telegram.updates where bot_id = $1 and update_id = $2",
    )
    .bind(BOT_ID)
    .bind(9_052_i64)
    .fetch_one(fixture.database.pool())
    .await
    .expect("update payload state");
    let capture_calls = state.capture_calls.load(Ordering::SeqCst);
    let keys = state
        .idempotency_keys
        .lock()
        .expect("key history lock")
        .clone();
    let bindings = binding_count(&fixture, operation).await;
    let intents: i64 = sqlx::query_scalar(
        "select count(*) from telegram.interaction_tokens
         where surface = 'deep_link' and action = 'operation_status' and operation_id = $1",
    )
    .bind(operation)
    .fetch_one(fixture.database.pool())
    .await
    .expect("operation intent count");
    let outbound = outbound_job_count(&fixture).await;

    sqlx::query("drop trigger reject_first_capture_ack_for_test on telegram.outbound_jobs")
        .execute(fixture.database.pool())
        .await
        .expect("the one-shot acknowledgement fault is removed");
    sqlx::query("drop function telegram.reject_first_capture_ack_for_test()")
        .execute(fixture.database.pool())
        .await
        .expect("the one-shot fault function is removed");
    sqlx::query("drop sequence telegram.capture_projection_fault_once_for_test")
        .execute(fixture.database.pool())
        .await
        .expect("the one-shot fault sequence is removed");
    let Fixture {
        database,
        app,
        blob_root,
    } = fixture;
    drop(app);
    drop(blob_root);
    database
        .cleanup()
        .await
        .expect("the disposable database is dropped before assertions");

    assert_eq!(
        (final_state.as_str(), payload_present),
        ("processed", false),
        "accepted capture must retry before terminal payload minimization"
    );
    assert_eq!(capture_calls, 2, "recovery replays the accepted command");
    assert_eq!(keys.len(), 2, "both submissions recorded their identity");
    assert_eq!(keys[0], keys[1], "recovery reuses the idempotency key");
    assert_eq!(bindings, 1, "recovery commits one binding");
    assert_eq!(intents, 1, "recovery commits one operation intent");
    assert_eq!(outbound, 1, "recovery enqueues one acknowledgement");
}

/// A capture retained for local-projection recovery must not monopolize the single worker: a
/// later unsupported update still reaches terminal settlement while the acknowledgement trigger
/// keeps the first capture processable.
#[tokio::test]
async fn persistent_capture_projection_fault_does_not_starve_later_update() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let (fixture, receiver, context) = Fixture::prepare(&base_url).await;
    let worker = tokio::spawn(intake::run_worker(
        fixture.database.database.clone(),
        receiver,
        Some(context),
    ));
    sqlx::query(
        "create function telegram.reject_persistent_capture_ack_for_test() returns trigger
         language plpgsql as $$
         begin
             raise exception 'synthetic persistent acknowledgement insertion failure';
         end
         $$",
    )
    .execute(fixture.database.pool())
    .await
    .expect("the persistent fault function is installed");
    sqlx::query(
        "create trigger reject_persistent_capture_ack_for_test
         before insert on telegram.outbound_jobs
         for each row
         when (new.kind = 'send_message' and new.operation_id is not null)
         execute function telegram.reject_persistent_capture_ack_for_test()",
    )
    .execute(fixture.database.pool())
    .await
    .expect("the persistent acknowledgement fault is installed");

    fixture
        .deliver(message_update(9_054, "https://example.test/article"))
        .await;
    fixture
        .deliver(message_update(9_055, "ordinary unsupported text"))
        .await;

    let later_state = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let state: String = sqlx::query_scalar(
                "select state from telegram.updates where bot_id = $1 and update_id = $2",
            )
            .bind(BOT_ID)
            .bind(9_055_i64)
            .fetch_one(fixture.database.pool())
            .await
            .expect("the later update remains queryable");
            if state == "unsupported" {
                return state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .ok();
    let first_processable: (String, bool) = sqlx::query_as(
        "select state, payload is not null
         from telegram.updates where bot_id = $1 and update_id = $2",
    )
    .bind(BOT_ID)
    .bind(9_054_i64)
    .fetch_one(fixture.database.pool())
    .await
    .expect("the first update remains queryable");

    worker.abort();
    let _ = worker.await;
    sqlx::query("drop trigger reject_persistent_capture_ack_for_test on telegram.outbound_jobs")
        .execute(fixture.database.pool())
        .await
        .expect("the persistent acknowledgement fault is removed");
    sqlx::query("drop function telegram.reject_persistent_capture_ack_for_test()")
        .execute(fixture.database.pool())
        .await
        .expect("the persistent fault function is removed");
    let Fixture {
        database,
        app,
        blob_root,
    } = fixture;
    drop(app);
    drop(blob_root);
    database
        .cleanup()
        .await
        .expect("the disposable database is dropped before assertions");

    assert_eq!(
        later_state.as_deref(),
        Some("unsupported"),
        "a retained capture must not starve a later unsupported update"
    );
    assert!(
        matches!(first_processable.0.as_str(), "accepted" | "processing") && first_processable.1,
        "the failed capture remains processable while the fault persists"
    );
    assert!(
        state.capture_calls.load(Ordering::SeqCst) >= 1,
        "the first capture reached Platform before its local projection failed"
    );
}

/// Two workers projecting concurrent accepted replays of one sender/source pair converge through
/// the operation binding authority and enqueue one acknowledgement.
#[tokio::test]
async fn concurrent_accepted_capture_recovery_enqueues_one_acknowledgement() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let database = TestDatabase::create().await.expect("disposable database");
    database
        .database
        .ensure_identity(OWNER_TELEGRAM_USER_ID, &IdentityProfile::default())
        .await
        .expect("the fixture owner identity");

    let (context, blob_root) = concurrent_capture_context(&base_url);

    let settings = IntakeSettings {
        secret: SecretString::new(SECRET.into()),
        max_body_bytes: 4096,
        bot_id: BOT_ID,
        queue_capacity: 2,
    };
    let (intake_state, mut receiver) = Intake::new(settings, database.database.clone());
    let app = intake_state.router();
    let request = |update: Value| {
        Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("x-telegram-bot-api-secret-token", SECRET)
            .body(axum::body::Body::from(update.to_string().into_bytes()))
            .expect("the request builds")
    };
    let (first_response, second_response) = tokio::join!(
        app.clone().oneshot(request(message_update(
            9_053,
            "https://example.test/article"
        ))),
        app.clone().oneshot(request(message_update(
            9_054,
            "https://example.test/article"
        ))),
    );
    let first_status = first_response
        .expect("the first admission responds")
        .status();
    let second_status = second_response
        .expect("the second admission responds")
        .status();
    let first = receiver.recv().await.expect("the first admitted update");
    let second = receiver.recv().await.expect("the second admitted update");

    tokio::join!(
        intake::process_one(&database.database, &first, Some(&context)),
        intake::process_one(&database.database, &second, Some(&context)),
    );

    let operation: uuid::Uuid = OPERATION_ID.parse().expect("synthetic uuid");
    let update_states: Vec<String> = sqlx::query_scalar(
        "select state from telegram.updates where update_id in ($1, $2) order by update_id",
    )
    .bind(9_053_i64)
    .bind(9_054_i64)
    .fetch_all(database.pool())
    .await
    .expect("both update states");
    let counts = projection_counts(&database, operation).await;
    let capture_calls = state.capture_calls.load(Ordering::SeqCst);
    let keys = state
        .idempotency_keys
        .lock()
        .expect("key history lock")
        .clone();

    drop(app);
    drop(intake_state);
    drop(context);
    drop(blob_root);
    database
        .cleanup()
        .await
        .expect("the disposable database is dropped before assertions");

    assert_eq!(first_status, HttpStatus::OK, "the first update is admitted");
    assert_eq!(
        second_status,
        HttpStatus::OK,
        "the second update is admitted"
    );
    assert_eq!(update_states, ["processed", "processed"]);
    assert_eq!(capture_calls, 2, "both accepted replays reached Platform");
    assert_eq!(keys.len(), 2, "both submissions recorded their identity");
    assert_eq!(keys[0], keys[1], "both submissions use one idempotency key");
    assert_eq!(counts.bindings, 1, "one operation binding wins");
    assert_eq!(counts.intents, 1, "one operation intent wins");
    assert_eq!(counts.outbound, 1, "one acknowledgement wins");
}
