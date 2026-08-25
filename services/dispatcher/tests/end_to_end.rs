//! End to end: the real Bot API client against a local fake server, the real durable queue, the
//! real consumer and sender. Nothing here contacts api.telegram.org.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::Path;
use axum::response::Json;
use axum::routing::any;
use bot_api::Client;
use common::{FakeClock, database};
use ratatoskr_telegram_dispatcher::outbound::sender::{ClientSink, OutboundSender, SenderLimits};
use ratatoskr_telegram_dispatcher::outbound::{Clock, DeliveryLimiter};
use ratatoskr_telegram_dispatcher::projection::{
    AcceptOutcome, OperationEvent, OperationStatus, ProjectionConsumer, SafeLine,
};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::types::Uuid;
use telegram_persistence::outbound_jobs::{MessagePayload, NewOutboundJob, OutboundJobKind};
use telegram_persistence::test_support::TestDatabase;
use url::Url;

/// A fixed synthetic instant: whole seconds since the Unix epoch, never read from a wall clock.
const T0: i64 = 1_800_000_000;
/// The bot identity the harness's `getMe` fixture carries.
const BOT_ID: i64 = 700_100_200;
/// The chat every projection lands in.
const CHAT: i64 = 900_700_601;
/// The bound Telegram message id the edits rewrite.
const BOUND_MESSAGE_ID: i64 = 100;
/// Synthetic credential. Never a real one.
const TOKEN: &str = "123456:TEST-e2e-harness-token";

/// One request the harness captured.
#[derive(Debug, Clone)]
struct Captured {
    /// The request path, e.g. `/bot<token>/editMessageText`.
    path: String,
    /// The parsed JSON body, carrying `text` for both methods this test drives.
    body: Option<Value>,
}

/// A local fake Bot API server: answers from a script one response per call, then falls back to a
/// server. The script maps CALL INDICES (zero-based, across all requests) to responses; every
/// other call gets a successful message result. Index keying keeps a scripted answer attached to
/// the call it was written for no matter what order earlier calls arrive in.
struct Harness {
    base_url: Url,
    captured: Arc<Mutex<Vec<Captured>>>,
}

impl Harness {
    /// Spawn on an ephemeral port. Serving runs on a dedicated runtime in its own thread, so the
    /// harness never nests a `block_on` inside the test's runtime.
    async fn spawn(script: Arc<Mutex<Vec<(usize, Value)>>>) -> Self {
        let captured: Arc<Mutex<Vec<Captured>>> = Arc::default();
        let state = Arc::clone(&captured);
        let app = Router::new().route(
            "/{*rest}",
            any(move |Path(path): Path<String>, body: axum::body::Bytes| {
                let state = Arc::clone(&state);
                let script = Arc::clone(&script);
                async move {
                    let script = script.lock().expect("script");
                    let index = state.lock().expect("capture lock").len();
                    let default = || {
                        json!({"ok": true, "result": {
                            "message_id": 555, "date": 1_760_000_000,
                            "chat": {"id": 900_700_601, "type": "private"},
                            "text": "harness default",
                        }})
                    };
                    let response = script
                        .iter()
                        .find(|(at, _)| *at == index)
                        .map_or_else(default, |(_, response)| response.clone());
                    drop(script);
                    state.lock().expect("capture lock").push(Captured {
                        path: format!("/{path}"),
                        body: serde_json::from_slice::<Value>(&body).ok(),
                    });
                    Json(response)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the harness binds port 0");
        let bound = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("harness runtime");
            let _ = runtime.block_on(axum::serve(listener, app).into_future());
        });
        Self {
            base_url: url::Url::parse(&format!("http://{bound}")).expect("base url"),
            captured,
        }
    }

    /// Every captured call so far, in arrival order.
    fn calls(&self) -> Vec<Captured> {
        self.captured.lock().expect("capture lock").clone()
    }
}

/// An authoritative pause, as Telegram sends it.
fn too_many_requests(retry_after: i64) -> Value {
    json!({"ok": false, "error_code": 429,
           "description": "Too Many Requests: retry after the parameter",
           "parameters": {"retry_after": retry_after}})
}

/// The successful-no-op answer for an identical edit.
fn not_modified() -> Value {
    json!({"ok": false, "error_code": 400,
           "description": "Bad Request: message is not modified"})
}

/// A running-stage event; tests override what they vary.
fn running_event(operation: Uuid, event_id: Uuid, occurred_at: i64, stage: &str) -> OperationEvent {
    OperationEvent {
        event_id,
        occurred_at_secs: occurred_at,
        correlation_id: format!("operation:{operation}"),
        operation_id: operation,
        status: OperationStatus::Running,
        stage: Some(stage.to_owned()),
        progress_percent: Some(40),
        errors: Vec::new(),
        warnings: vec![SafeLine {
            code: "w.tick".to_owned(),
            message: "tick".to_owned(),
        }],
    }
}

/// The full production composition over one database and one harness endpoint, sharing one
/// injected clock between the consumer's accepts and the sender's eligibility checks.
fn composed(
    db: &TestDatabase,
    harness: &Harness,
    clock: &Arc<dyn Clock>,
) -> (ProjectionConsumer, OutboundSender) {
    let client = Client::new(
        &SecretString::new(TOKEN.to_owned().into()),
        &harness.base_url,
        Duration::from_secs(5),
    )
    .expect("the harness client must build");
    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    let sender = OutboundSender::new(
        Arc::new(db.database.clone()),
        Arc::new(ClientSink::new(client)),
        limiter,
        clock.clone(),
        SenderLimits {
            max_attempts: 5,
            backoff_base_secs: 1,
            backoff_cap_secs: 60,
            jitter_fraction_milli: 0,
            lease_ttl_secs: 30,
        },
    );
    let consumer = ProjectionConsumer::new(db.database.clone(), clock.clone(), 4);
    (consumer, sender)
}

/// Seed the binding so edits have a target: message 100 acknowledged at [`T0`].
async fn seed_binding(db: &TestDatabase, operation: Uuid) {
    db.database
        .ensure_operation_binding(BOT_ID, operation, CHAT)
        .await
        .expect("ensure binding");
    db.database
        .record_send_acknowledged(BOT_ID, operation, CHAT, BOUND_MESSAGE_ID, T0)
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

/// The whole lifecycle through the real seams: four revisions delivered in acceptance order (one
/// of them across an authoritative 429 pause, one answered as a successful no-op), a duplicate
/// and a stale event dropped without traffic, exactly one Completed render, and a queue that ends
/// empty with the binding intact.
#[tokio::test]
async fn operation_lifecycle_renders_progress_then_terminal_once_through_fake_bot_api() {
    let db = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&db, operation).await;

    // Call 1 (the second wire call) is revision 2's first attempt and meets the 429; call 3 is
    // revision 3 and meets the successful no-op.
    let script = Arc::new(Mutex::new(vec![
        (1, too_many_requests(1)),
        (3, not_modified()),
    ]));
    let harness = Harness::spawn(script).await;
    let fake_clock = FakeClock::at(T0);
    let clock: Arc<dyn Clock> = fake_clock.clone();
    let (consumer, sender) = composed(&db, &harness, &clock);

    let downloading = running_event(operation, Uuid::now_v7(), T0 + 2, "downloading");
    drive_lifecycle(&consumer, &sender, &fake_clock, operation, &downloading).await;

    // The wire saw exactly five calls: four revisions plus the one authorized 429 retry.
    let calls = harness.calls();
    assert_eq!(calls.len(), 5, "every wire call: {calls:?}");
    for call in &calls {
        // teloxide spells paths with the Bot API's own method-name casing.
        assert!(
            call.path.to_lowercase().ends_with("/editmessagetext"),
            "an all-edit lifecycle must never send fresh messages: {}",
            call.path
        );
    }

    // Successful texts appear in exactly the accepted order; the retried revision repeats its
    // own body (at-least-once), and nothing else reaches the wire twice.
    let texts: Vec<String> = calls
        .iter()
        .filter_map(|call| {
            call.body
                .as_ref()
                .and_then(|body| body.get("text"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let mut deduped_consecutive: Vec<String> = Vec::new();
    for text in &texts {
        if deduped_consecutive.last().map(String::as_str) != Some(text.as_str()) {
            deduped_consecutive.push(text.clone());
        }
    }
    assert_eq!(
        deduped_consecutive,
        vec![
            "<b>Accepted</b> \u{2014} 40%\n- (w.tick) tick".to_owned(),
            "<b>In progress</b> \u{2014} 40%\ndownloading\n- (w.tick) tick".to_owned(),
            "<b>In progress</b> \u{2014} 40%\nextracting\n- (w.tick) tick".to_owned(),
            "<b>Completed</b>\n- (w.tick) tick".to_owned(),
        ],
        "renders reach the wire in acceptance order"
    );
    assert_eq!(
        texts
            .iter()
            .filter(|text| text.contains("Completed"))
            .count(),
        1,
        "exactly one Completed render ever goes out"
    );

    // Final state: every job settled, the binding advanced but never rebound.
    assert_eq!(
        job_counts(&db, operation).await,
        vec![("sent".to_owned(), 4)],
        "all four revisions end sent"
    );
    let binding = db
        .database
        .find_binding(operation, CHAT)
        .await
        .expect("find")
        .expect("binding exists");
    assert_eq!(binding.last_rendered_revision, 4);
    assert_eq!(binding.message_id, Some(BOUND_MESSAGE_ID));

    db.cleanup().await.expect("cleanup");
}

/// Drive the six-step lifecycle: accept then deliver for each revision, with the 429 pause, the
/// not-modified no-op, a duplicate envelope, and a stale event in the middle.
async fn drive_lifecycle(
    consumer: &ProjectionConsumer,
    sender: &OutboundSender,
    clock: &FakeClock,
    operation: Uuid,
    downloading: &OperationEvent,
) {
    // Revision 1: the accepted snapshot renders immediately (nothing delivered yet).
    let mut accepted = running_event(operation, Uuid::now_v7(), T0 + 1, "starting");
    accepted.status = OperationStatus::Accepted;
    accepted.stage = None;
    assert_eq!(
        consumer.accept(&accepted).await.expect("accept"),
        AcceptOutcome::Recorded
    );
    assert!(sender.run_once().await.expect("run"), "revision 1 is due");

    // Revision 2: throttled past the delivered render, then paused by a 429 and recovered after
    // its authoritative delay.
    clock.advance_secs(5);
    assert_eq!(
        consumer.accept(downloading).await.expect("accept"),
        AcceptOutcome::Recorded
    );
    assert!(
        sender.run_once().await.expect("run"),
        "revision 2 reaches the wire into the 429"
    );
    clock.advance_secs(2);
    assert!(
        sender.run_once().await.expect("run"),
        "revision 2 recovers after the pause"
    );

    // Revision 3: answered `message is not modified` — a successful no-op that still advances.
    clock.advance_secs(5);
    let extracting = running_event(operation, Uuid::now_v7(), T0 + 3, "extracting");
    assert_eq!(
        consumer.accept(&extracting).await.expect("accept"),
        AcceptOutcome::Recorded
    );
    assert!(sender.run_once().await.expect("run"));

    // A redelivered envelope changes nothing twice; an older event drops without effect.
    assert_eq!(
        consumer.accept(downloading).await.expect("duplicate"),
        AcceptOutcome::Duplicate
    );
    assert_eq!(
        consumer
            .accept(&running_event(operation, Uuid::now_v7(), T0 + 1, "late"))
            .await
            .expect("stale"),
        AcceptOutcome::Stale
    );

    // Revision 4: the terminal renders once, skipping the interval delay but not the queue.
    clock.advance_secs(5);
    let mut done = running_event(operation, Uuid::now_v7(), T0 + 4, "finishing");
    done.status = OperationStatus::Succeeded;
    done.stage = None;
    done.progress_percent = None;
    assert_eq!(
        consumer.accept(&done).await.expect("terminal"),
        AcceptOutcome::Recorded
    );
    assert!(sender.run_once().await.expect("run"));
    assert!(
        !sender.run_once().await.expect("drained"),
        "the queue must be empty after the terminal render"
    );
}

/// A job enqueued but never delivered survives a "process restart": a freshly constructed sender
/// over the same database picks it up and delivers it exactly once.
#[tokio::test]
async fn a_job_enqueued_but_undelivered_survives_process_restart() {
    let db = database().await;
    db.database
        .enqueue_outbound_job(
            &NewOutboundJob {
                bot_id: BOT_ID,
                chat_id: 700_500_400,
                kind: OutboundJobKind::SendMessage,
                payload: MessagePayload::text("written before the crash"),
                content_hash: "hash-before-crash".to_owned(),
                operation_id: None,
                revision: None,
                correlation_id: None,
                next_attempt_at: Some(T0),
            },
            T0,
        )
        .await
        .expect("enqueue");

    // No sender runs. A NEW process equivalent: fresh components over the same database.
    let script: Arc<Mutex<Vec<(usize, Value)>>> = Arc::new(Mutex::new(Vec::new()));
    let harness = Harness::spawn(script).await;
    let client = Client::new(
        &SecretString::new(TOKEN.to_owned().into()),
        &harness.base_url,
        Duration::from_secs(5),
    )
    .expect("the harness client must build");
    let restarted = OutboundSender::new(
        Arc::new(db.database.clone()),
        Arc::new(ClientSink::new(client)),
        Arc::new(DeliveryLimiter::new(30, 0)),
        FakeClock::at(T0),
        SenderLimits {
            max_attempts: 5,
            backoff_base_secs: 1,
            backoff_cap_secs: 60,
            jitter_fraction_milli: 0,
            lease_ttl_secs: 30,
        },
    );

    assert!(
        restarted.run_once().await.expect("the restart delivers"),
        "the orphaned job must be claimable without any external trigger"
    );
    assert!(
        !restarted.run_once().await.expect("drained"),
        "delivered exactly once"
    );

    let calls = harness.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(
        calls[0].path.to_lowercase().ends_with("/sendmessage"),
        "the restart delivers a fresh sendMessage: {}",
        calls[0].path
    );
    let delivered = calls[0]
        .body
        .as_ref()
        .and_then(|body| body.get("text"))
        .and_then(Value::as_str);
    assert_eq!(delivered, Some("written before the crash"));

    db.cleanup().await.expect("cleanup");
}
