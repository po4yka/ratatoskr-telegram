//! The outbound sender against a real `PostgreSQL` queue and a recording fake Bot API.
//!
//! Every test owns a disposable database and an injected clock, so ordering, backoff, and
//! cooldown behavior are deterministic: no test sleeps, and no test talks to Telegram.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::collections::VecDeque;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bot_api::BotApiError;
use ratatoskr_telegram_dispatcher::outbound::DeliveryLimiter;
use ratatoskr_telegram_dispatcher::outbound::sender::{
    BotApiSink, OutboundSender, SendFuture, SenderLimits, SentMessage,
};
use telegram_persistence::outbound_jobs::{MessagePayload, NewOutboundJob, OutboundJobKind};
use telegram_persistence::test_support::TestDatabase;
mod common;

use common::{FakeClock, database};
use uuid::Uuid;

/// A fixed synthetic instant: whole seconds since the Unix epoch, never read from a wall clock.
const T0: i64 = 1_800_000_000;
/// The one bot every synthetic job belongs to.
const BOT_ID: i64 = 700_100_200;

/// One observed Bot API call, in observation order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CallRecord {
    kind: &'static str,
    chat_id: i64,
    message_id: Option<i64>,
    payload: MessagePayload,
}

/// A mid-flight block: the fake signals `started_tx` when the call begins and waits on
/// `release_rx` before answering, so a test can observe the world while a job is in flight.
#[derive(Debug)]
struct Gate {
    started_tx: tokio::sync::oneshot::Sender<()>,
    release_rx: tokio::sync::oneshot::Receiver<()>,
}

/// The recording fake behind the sender seam: replays a fault queue one answer per call, then
/// succeeds with fresh message ids. Tracks the in-flight maximum so overlap can be asserted.
#[derive(Debug)]
struct FakeBotApi {
    records: Mutex<Vec<CallRecord>>,
    faults: Mutex<VecDeque<BotApiError>>,
    gate: Mutex<Option<Gate>>,
    next_message_id: AtomicI64,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

impl FakeBotApi {
    fn new(faults: VecDeque<BotApiError>) -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(Vec::new()),
            faults: Mutex::new(faults),
            gate: Mutex::new(None),
            next_message_id: AtomicI64::new(1000),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
        })
    }

    fn records(&self) -> Vec<CallRecord> {
        self.records.lock().expect("records").clone()
    }

    fn arm_gate(
        &self,
        started_tx: tokio::sync::oneshot::Sender<()>,
        release_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        *self.gate.lock().expect("gate") = Some(Gate {
            started_tx,
            release_rx,
        });
    }

    fn perform(
        &self,
        kind: &'static str,
        chat_id: i64,
        message_id: Option<i64>,
        payload: &MessagePayload,
    ) -> SendFuture<'_> {
        let payload = payload.clone();
        Box::pin(async move {
            let current = self.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
            self.max_in_flight.fetch_max(current, Ordering::Relaxed);

            // The guard must drop before the await: a std MutexGuard is !Send and would poison
            // the boxed future.
            let gate = self.gate.lock().expect("gate").take();
            if let Some(gate) = gate {
                let _ = gate.started_tx.send(());
                let _ = gate.release_rx.await;
            }

            let mut records = self.records.lock().expect("records");
            records.push(CallRecord {
                kind,
                chat_id,
                message_id,
                payload,
            });
            drop(records);

            let result = match self.faults.lock().expect("faults").pop_front() {
                Some(error) => Err(error),
                None => Ok(SentMessage {
                    message_id: self.next_message_id.fetch_add(1, Ordering::Relaxed),
                }),
            };
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
            result
        })
    }
}

impl BotApiSink for FakeBotApi {
    fn send_message(&self, chat_id: i64, payload: &MessagePayload) -> SendFuture<'_> {
        self.perform("send_message", chat_id, None, payload)
    }

    fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        payload: &MessagePayload,
    ) -> SendFuture<'_> {
        self.perform("edit_message_text", chat_id, Some(message_id), payload)
    }
}

/// Sender limits with deterministic timing: zero jitter fraction, one-second backoff base.
fn limits(max_attempts: u32) -> SenderLimits {
    SenderLimits {
        max_attempts,
        backoff_base_secs: 1,
        backoff_cap_secs: 60,
        jitter_fraction_milli: 0,
        lease_ttl_secs: 30,
    }
}

fn make_sender(
    db: &TestDatabase,
    sink: Arc<FakeBotApi>,
    clock: Arc<FakeClock>,
    limiter: Arc<DeliveryLimiter>,
    max_attempts: u32,
) -> OutboundSender {
    OutboundSender::new(
        Arc::new(db.database.clone()),
        sink,
        limiter,
        clock,
        limits(max_attempts),
    )
}

/// Enqueue one job due immediately at [`T0`].
async fn enqueue(
    db: &TestDatabase,
    chat_id: i64,
    kind: OutboundJobKind,
    body: &str,
    operation_id: Option<Uuid>,
    revision: Option<i64>,
) -> Uuid {
    db.database
        .enqueue_outbound_job(
            &NewOutboundJob {
                bot_id: BOT_ID,
                chat_id,
                kind,
                payload: MessagePayload::text(body),
                content_hash: format!("hash-{body}"),
                operation_id,
                revision,
                correlation_id: None,
                next_attempt_at: Some(T0),
            },
            T0,
        )
        .await
        .expect("enqueue")
}

/// `(state, last_error_class, next_attempt_epoch)` of one job row.
async fn job_row(db: &TestDatabase, id: Uuid) -> (String, Option<String>, Option<i64>) {
    sqlx::query_as(
        "select state, last_error_class, extract(epoch from next_attempt_at)::bigint
         from telegram.outbound_jobs
         where id = $1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .expect("job row")
}

/// A provider response that proves the request was refused but is not a known permanent class.
fn definite_transient_refusal() -> BotApiError {
    BotApiError::Api {
        description: "Internal Server Error: retry later".to_owned(),
    }
}

/// Chat A delivers its three jobs strictly in enqueue order while chats B and C interleave, two
/// concurrent sender loops drain the queue, and no two calls are ever in flight simultaneously.
///
/// Pinned here: per-chat FIFO end to end (the claim query's guarantee observed on the wire), the
/// one-job-in-flight invariant across concurrent loops, and cross-chat progress (B and C are not
/// starved by A's backlog). Cross-chat ORDER is deliberately not pinned — the spec promises none.
#[tokio::test]
async fn sender_delivers_one_chat_fifo_under_concurrency() {
    let db = database().await;
    for body in ["A1", "A2", "A3"] {
        enqueue(&db, 100, OutboundJobKind::SendMessage, body, None, None).await;
    }
    enqueue(&db, 200, OutboundJobKind::SendMessage, "B1", None, None).await;
    enqueue(&db, 300, OutboundJobKind::SendMessage, "C1", None, None).await;

    let fake = FakeBotApi::new(VecDeque::new());
    let clock = FakeClock::at(T0);
    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    // Generous bound: a limiter deferral consumes an attempt slot by design, and the static
    // early ticks make deferrals likely before the clock steps forward.
    let sender = Arc::new(make_sender(
        &db,
        Arc::clone(&fake),
        Arc::clone(&clock),
        Arc::clone(&limiter),
        10,
    ));

    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let sender = Arc::clone(&sender);
        let fake = Arc::clone(&fake);
        let clock = Arc::clone(&clock);
        workers.spawn(async move {
            let mut attempts_left = 200u32;
            while fake.records().len() < 5 && attempts_left > 0 {
                attempts_left -= 1;
                // One tick of injected time per attempt stands in for the idle poll: it refills
                // the single-token burst so deferred jobs become deliverable again.
                clock.advance_secs(1);
                sender.run_once().await.expect("run_once");
            }
        });
    }
    while let Some(worker) = workers.join_next().await {
        worker.expect("worker task");
    }

    let records = fake.records();
    assert_eq!(records.len(), 5, "every job reaches the wire exactly once");

    let a_records: Vec<&CallRecord> = records
        .iter()
        .filter(|record| record.chat_id == 100)
        .collect();
    assert_eq!(
        a_records
            .iter()
            .map(|record| record.payload.text.as_str())
            .collect::<Vec<_>>(),
        ["A1", "A2", "A3"],
        "chat A must reach the wire in enqueue order"
    );

    let mut delivered: Vec<(i64, &str)> = records
        .iter()
        .map(|record| (record.chat_id, record.payload.text.as_str()))
        .collect();
    delivered.sort_unstable();
    assert_eq!(
        delivered,
        vec![
            (100, "A1"),
            (100, "A2"),
            (100, "A3"),
            (200, "B1"),
            (300, "C1")
        ],
        "each job delivered exactly once across both loops"
    );

    assert_eq!(
        fake.max_in_flight.load(Ordering::Relaxed),
        1,
        "no two Bot API calls may ever be in flight simultaneously"
    );
}

/// A `429` carrying `retry_after: 30` reschedules the job at now+30 (zero jitter), cools the chat
/// through the limiter penalty, produces no earlier reattempt, and recovers once the deadline
/// passes.
#[tokio::test]
async fn rate_limited_answer_reschedules_job_and_cools_chat() {
    let db = database().await;
    let job = enqueue(
        &db,
        400,
        OutboundJobKind::SendMessage,
        "limited",
        None,
        None,
    )
    .await;

    let fake = FakeBotApi::new(VecDeque::from([BotApiError::RateLimited {
        retry_after: Duration::from_secs(30),
    }]));
    let clock = FakeClock::at(T0);
    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    let sender = make_sender(
        &db,
        Arc::clone(&fake),
        Arc::clone(&clock),
        Arc::clone(&limiter),
        5,
    );

    assert!(sender.run_once().await.expect("first run"));

    let (state, _, next_attempt) = job_row(&db, job).await;
    assert_eq!(
        state, "retry_wait",
        "a rate-limited job waits, it does not die"
    );
    assert_eq!(next_attempt, Some(T0 + 30), "retry_after is authoritative");

    assert!(
        matches!(
            limiter.try_acquire(clock.as_ref(), 400),
            ratatoskr_telegram_dispatcher::outbound::RateDecision::ChatWait { after_ms } if after_ms == 30_000
        ),
        "the chat must be cooled until the retry_after deadline"
    );

    assert!(
        !sender.run_once().await.expect("second run"),
        "no reattempt before the deadline: the row is not due"
    );

    clock.advance_secs(31);
    assert!(sender.run_once().await.expect("third run"));
    let (state, _, _) = job_row(&db, job).await;
    assert_eq!(state, "sent", "the job recovers after the deadline passes");
    assert_eq!(fake.records().len(), 2, "exactly one retry after the pause");
}

/// Transient failures back off between attempts and dead-letter at the attempt bound with the
/// `transient` class recorded.
#[tokio::test]
async fn transient_failure_backs_off_then_dead_letters_at_bound() {
    let db = database().await;
    let job = enqueue(&db, 500, OutboundJobKind::SendMessage, "flaky", None, None).await;

    let fake = FakeBotApi::new(VecDeque::from([
        definite_transient_refusal(),
        definite_transient_refusal(),
    ]));
    let clock = FakeClock::at(T0);
    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    let sender = make_sender(
        &db,
        Arc::clone(&fake),
        Arc::clone(&clock),
        Arc::clone(&limiter),
        2,
    );

    assert!(sender.run_once().await.expect("first run"));
    let (state, _, next_attempt) = job_row(&db, job).await;
    assert_eq!(
        state, "retry_wait",
        "the first transient failure reschedules"
    );
    assert_eq!(next_attempt, Some(T0 + 1), "backoff starts at the base");

    clock.advance_secs(2);
    assert!(sender.run_once().await.expect("second run"));
    let (state, class, _) = job_row(&db, job).await;
    assert_eq!(state, "failed_permanent", "the bound dead-letters the job");
    assert_eq!(
        class.as_deref(),
        Some("transient"),
        "exhausted transients carry their class"
    );
    assert_eq!(
        fake.records().len(),
        2,
        "exactly the bound number of attempts"
    );

    assert!(
        !sender.run_once().await.expect("third run"),
        "a dead-lettered job is never claimed again"
    );
}

/// A permanent failure settles `failed_permanent` after exactly ONE wire call; nothing retries.
#[tokio::test]
async fn permanent_failure_settles_once_without_retry() {
    let db = database().await;
    let job = enqueue(
        &db,
        600,
        OutboundJobKind::SendMessage,
        "blocked",
        None,
        None,
    )
    .await;

    let fake = FakeBotApi::new(VecDeque::from([BotApiError::Api {
        description: "Forbidden: bot was blocked by the user".to_owned(),
    }]));
    let clock = FakeClock::at(T0);
    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    let sender = make_sender(
        &db,
        Arc::clone(&fake),
        Arc::clone(&clock),
        Arc::clone(&limiter),
        5,
    );

    assert!(sender.run_once().await.expect("first run"));

    let (state, class, _) = job_row(&db, job).await;
    assert_eq!(state, "failed_permanent");
    assert_eq!(
        class.as_deref(),
        Some("bot_blocked"),
        "the safe class label is stored"
    );
    assert_eq!(fake.records().len(), 1, "a permanent failure never retries");

    assert!(
        !sender.run_once().await.expect("second run"),
        "nothing is left to claim after the immediate dead-letter"
    );
}

/// The bounded delivery metric vocabulary appears in the exposition after real outcomes, and no
/// chat id or message body text ever does: labels are closed classes only.
#[test]
fn delivery_outcomes_are_countable_without_content() {
    static GUARD: OnceLock<telegram_telemetry::TelemetryGuard> = OnceLock::new();
    let guard = GUARD.get_or_init(|| {
        telegram_telemetry::init(
            &telegram_core::config::TelemetryConfig::default(),
            telegram_core::RuntimeRole::Dispatcher,
        )
        .expect("the registry installs once per process")
    });

    let runtime = tokio::runtime::Runtime::new().expect("metrics runtime");
    runtime.block_on(async {
        let db = database().await;
        let clock = FakeClock::at(T0);
        let limiter = Arc::new(DeliveryLimiter::new(30, 0));

        // One transient retry that then succeeds.
        enqueue(
            &db,
            918_273_645,
            OutboundJobKind::SendMessage,
            "qzx-canary-transient",
            None,
            None,
        )
        .await;
        let fake = FakeBotApi::new(VecDeque::from([definite_transient_refusal()]));
        let sender = make_sender(
            &db,
            Arc::clone(&fake),
            Arc::clone(&clock),
            Arc::clone(&limiter),
            5,
        );
        assert!(sender.run_once().await.expect("transient attempt"));
        clock.advance_secs(2);
        assert!(sender.run_once().await.expect("transient recovery"));

        // One permanent failure with its safe class label. A fresh tick refills the
        // shared single-token burst so this delivery is not deferred instead.
        clock.advance_secs(1);
        enqueue(
            &db,
            192_837_465,
            OutboundJobKind::SendMessage,
            "qzx-canary-permanent",
            None,
            None,
        )
        .await;
        let fake = FakeBotApi::new(VecDeque::from([BotApiError::Api {
            description: "Forbidden: bot was blocked by the user".to_owned(),
        }]));
        let sender = make_sender(&db, fake, Arc::clone(&clock), Arc::clone(&limiter), 5);
        assert!(sender.run_once().await.expect("permanent attempt"));

        // One authoritative rate-limit pause.
        clock.advance_secs(1);
        enqueue(
            &db,
            555_444_333,
            OutboundJobKind::SendMessage,
            "qzx-canary-limited",
            None,
            None,
        )
        .await;
        let fake = FakeBotApi::new(VecDeque::from([BotApiError::RateLimited {
            retry_after: Duration::from_secs(30),
        }]));
        let sender = make_sender(&db, fake, Arc::clone(&clock), Arc::clone(&limiter), 5);
        assert!(sender.run_once().await.expect("limited attempt"));

        // The queue-depth gauge samples whatever is left (the rate-limited job waits).
        ratatoskr_telegram_dispatcher::outbound::sender::record_queue_depth(&db.database).await;

        db.cleanup().await.expect("cleanup");
    });

    let exposition = guard.metrics_handle().render();
    for series in [
        "telegram_delivery_retries_total{class=\"transient\"}",
        "telegram_delivery_failures_total{class=\"bot_blocked\"}",
        "telegram_rate_limit_waits_total",
        "telegram_delivery_duration_seconds",
        "telegram_outbound_queue_depth{state=\"retry_wait\"}",
    ] {
        assert!(
            exposition.contains(series),
            "{series} missing from:\n{exposition}"
        );
    }
    for canary in ["918273645", "192837465", "555444333", "qzx-canary"] {
        assert!(
            !exposition.contains(canary),
            "the exposition leaked content: {canary} appears in:\n{exposition}"
        );
    }
}

/// `message is not modified` settles `sent` and advances the binding's rendered revision even
/// though the bytes did not change; the bound message id stays untouched.
#[tokio::test]
async fn not_modified_answer_settles_sent_and_advances_revision() {
    let db = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&db, operation, 700, 42, 4).await;

    let job = enqueue(
        &db,
        700,
        OutboundJobKind::EditMessageText,
        "revision five",
        Some(operation),
        Some(5),
    )
    .await;

    let fake = FakeBotApi::new(VecDeque::from([BotApiError::Api {
        description: "Bad Request: message is not modified".to_owned(),
    }]));
    let clock = FakeClock::at(T0);
    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    let sender = make_sender(
        &db,
        Arc::clone(&fake),
        Arc::clone(&clock),
        Arc::clone(&limiter),
        5,
    );

    assert!(sender.run_once().await.expect("run"));

    let (state, _, _) = job_row(&db, job).await;
    assert_eq!(state, "sent", "message-not-modified is a successful no-op");

    let binding = db
        .database
        .find_binding(operation, 700)
        .await
        .expect("find")
        .expect("binding exists");
    assert_eq!(binding.last_rendered_revision, 5, "the revision advances");
    assert_eq!(binding.last_rendered_at, Some(T0), "the render stamp moves");
    assert_eq!(
        binding.message_id,
        Some(42),
        "the bound message id never moved"
    );

    let records = fake.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "edit_message_text");
    assert_eq!(records[0].message_id, Some(42));
}

/// An edit whose revision is not newer than the binding's last rendered one is marked superseded
/// before any wire call happens.
#[tokio::test]
async fn stale_edit_superseded_before_the_wire() {
    let db = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&db, operation, 900, 42, 6).await;

    let job = enqueue(
        &db,
        900,
        OutboundJobKind::EditMessageText,
        "stale revision",
        Some(operation),
        Some(4),
    )
    .await;

    let fake = FakeBotApi::new(VecDeque::new());
    let clock = FakeClock::at(T0);
    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    let sender = make_sender(
        &db,
        Arc::clone(&fake),
        Arc::clone(&clock),
        Arc::clone(&limiter),
        5,
    );

    assert!(sender.run_once().await.expect("run"));

    let (state, _, _) = job_row(&db, job).await;
    assert_eq!(state, "superseded", "the stale revision is withdrawn");
    assert!(
        fake.records().is_empty(),
        "a stale edit must be caught before the wire"
    );
}

/// No binding row may exist while a send is still in flight; the binding appears only after the
/// Bot API acknowledged, carrying exactly the returned message id.
#[tokio::test]
async fn send_ack_creates_binding_only_after_success() {
    let db = database().await;
    let operation = Uuid::now_v7();
    enqueue(
        &db,
        800,
        OutboundJobKind::SendMessage,
        "fresh news",
        Some(operation),
        None,
    )
    .await;

    let fake = FakeBotApi::new(VecDeque::new());
    let clock = FakeClock::at(T0);
    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    let sender = Arc::new(make_sender(
        &db,
        Arc::clone(&fake),
        Arc::clone(&clock),
        Arc::clone(&limiter),
        5,
    ));

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    fake.arm_gate(started_tx, release_rx);

    let task = tokio::spawn(async move { sender.run_once().await.expect("run") });
    started_rx.await.expect("the fake must be reached");

    assert!(
        db.database
            .find_binding(operation, 800)
            .await
            .expect("find")
            .is_none(),
        "no binding may exist while the send is still in flight"
    );

    release_tx.send(()).expect("gate release");
    assert!(task.await.expect("task"), "the gated send completes");

    let binding = db
        .database
        .find_binding(operation, 800)
        .await
        .expect("find")
        .expect("the ack creates the binding");
    assert_eq!(
        binding.message_id,
        Some(1000),
        "the fake's returned id is stored"
    );
}

/// A permanent edit failure dead-letters, clears the binding's message id, and the next revision
/// goes out as a fresh SEND that rebinds the new message id.
#[tokio::test]
async fn permanent_edit_failure_unbinds_and_next_revision_resends() {
    let db = database().await;
    let operation = Uuid::now_v7();
    seed_binding(&db, operation, 950, 42, 3).await;

    let dead_job = enqueue(
        &db,
        950,
        OutboundJobKind::EditMessageText,
        "revision four",
        Some(operation),
        Some(4),
    )
    .await;

    let fake = FakeBotApi::new(VecDeque::from([BotApiError::Api {
        description: "Bad Request: message to edit not found".to_owned(),
    }]));
    let clock = FakeClock::at(T0);
    let limiter = Arc::new(DeliveryLimiter::new(30, 0));
    let sender = make_sender(
        &db,
        Arc::clone(&fake),
        Arc::clone(&clock),
        Arc::clone(&limiter),
        5,
    );

    assert!(sender.run_once().await.expect("first run"));
    let (state, class, _) = job_row(&db, dead_job).await;
    assert_eq!(state, "failed_permanent");
    assert_eq!(class.as_deref(), Some("edit_target_gone"));

    let binding = db
        .database
        .find_binding(operation, 950)
        .await
        .expect("find")
        .expect("the row survives");
    assert_eq!(binding.message_id, None, "the dead target is unbound");

    let retry_job = enqueue(
        &db,
        950,
        OutboundJobKind::EditMessageText,
        "revision five",
        Some(operation),
        Some(5),
    )
    .await;

    // The first delivery spent the single-token burst at T0; one injected tick refills it.
    clock.advance_secs(1);
    assert!(sender.run_once().await.expect("second run"));

    let records = fake.records();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].kind, "edit_message_text",
        "the doomed revision edited"
    );
    assert_eq!(
        records[1].kind, "send_message",
        "after an unbind the next revision sends fresh instead of editing"
    );

    let (state, _, _) = job_row(&db, retry_job).await;
    assert_eq!(state, "sent");

    let binding = db
        .database
        .find_binding(operation, 950)
        .await
        .expect("find")
        .expect("binding exists");
    assert_eq!(binding.message_id, Some(1000), "the fresh send rebinds");
    assert_eq!(
        binding.last_rendered_revision, 5,
        "and its revision renders"
    );
}

/// Create a binding at `(operation, chat)` carrying `message_id` and `revision`, stamped before
/// [`T0`].
async fn seed_binding(
    db: &TestDatabase,
    operation: Uuid,
    chat_id: i64,
    message_id: i64,
    revision: i64,
) {
    db.database
        .ensure_operation_binding(BOT_ID, operation, chat_id)
        .await
        .expect("ensure binding");
    db.database
        .record_send_acknowledged(BOT_ID, operation, chat_id, message_id, T0 - 10)
        .await
        .expect("ack");
    db.database
        .advance_render(operation, chat_id, revision, T0 - 10)
        .await
        .expect("advance");
}
