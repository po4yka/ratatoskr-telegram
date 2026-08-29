//! Non-idempotent send ambiguity and known-acknowledgement recovery.

#![allow(clippy::expect_used, reason = "assertions in a test binary")]

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bot_api::BotApiError;
use ratatoskr_telegram_dispatcher::outbound::sender::{
    AcknowledgementFuture, AcknowledgementStore, BotApiSink, OutboundSender, SendFuture,
    SenderLimits, SentMessage,
};
use ratatoskr_telegram_dispatcher::outbound::{Clock, DeliveryLimiter};
use telegram_persistence::outbound_jobs::{
    AcknowledgedMethod, MessagePayload, NewOutboundJob, OutboundJobKind, QueuedOutboundJob,
};
use telegram_persistence::{Database, PersistenceError};

mod common;
use common::{FakeClock, database};

const T0: i64 = 1_800_000_000;
const BOT_ID: i64 = 700_100_200;

#[derive(Debug)]
struct RecordingBotApi {
    faults: Mutex<Vec<BotApiError>>,
    calls: AtomicUsize,
    next_message_id: AtomicI64,
}

impl RecordingBotApi {
    fn new(faults: Vec<BotApiError>) -> Arc<Self> {
        Arc::new(Self {
            faults: Mutex::new(faults),
            calls: AtomicUsize::new(0),
            next_message_id: AtomicI64::new(1000),
        })
    }
}

impl BotApiSink for RecordingBotApi {
    fn send_message(&self, _chat_id: i64, _payload: &MessagePayload) -> SendFuture<'_> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let fault = self.faults.lock().expect("faults").pop();
            if let Some(error) = fault {
                return Err(error);
            }
            Ok(SentMessage {
                message_id: self.next_message_id.fetch_add(1, Ordering::SeqCst),
            })
        })
    }

    fn edit_message_text(
        &self,
        chat_id: i64,
        _message_id: i64,
        payload: &MessagePayload,
    ) -> SendFuture<'_> {
        self.send_message(chat_id, payload)
    }
}

#[derive(Debug)]
struct OneShotFaultAcknowledgementStore {
    database: Database,
    calls: AtomicUsize,
}

impl AcknowledgementStore for OneShotFaultAcknowledgementStore {
    fn record<'a>(
        &'a self,
        job: &'a QueuedOutboundJob,
        method: AcknowledgedMethod,
        message_id: i64,
        now: i64,
    ) -> AcknowledgementFuture<'a> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(PersistenceError::StaleOutboundAcknowledgement);
            }
            self.database
                .record_outbound_acknowledgement(job, method, message_id, now)
                .await
        })
    }
}

fn limits() -> SenderLimits {
    SenderLimits {
        max_attempts: 5,
        backoff_base_secs: 1,
        backoff_cap_secs: 60,
        jitter_fraction_milli: 0,
        lease_ttl_secs: 30,
    }
}

async fn enqueue(db: &Database, chat_id: i64, body: &str) -> uuid::Uuid {
    db.enqueue_outbound_job(
        &NewOutboundJob {
            bot_id: BOT_ID,
            chat_id,
            kind: OutboundJobKind::SendMessage,
            payload: MessagePayload::text(body),
            content_hash: format!("hash-{body}"),
            operation_id: None,
            revision: None,
            correlation_id: None,
            next_attempt_at: Some(T0),
        },
        T0,
    )
    .await
    .expect("enqueue")
}

async fn state(db: &Database, id: uuid::Uuid) -> (String, Option<String>) {
    sqlx::query_as("select state, last_error_class from telegram.outbound_jobs where id = $1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .expect("job row")
}

#[tokio::test]
async fn transport_timeout_enters_outcome_unknown_without_retry() {
    let db = database().await;
    let job = enqueue(&db.database, 550, "ambiguous").await;
    let fake = RecordingBotApi::new(vec![BotApiError::Io(Arc::new(std::io::Error::other(
        "synthetic transport failure",
    )))]);
    let clock = FakeClock::at(T0);
    let sender = OutboundSender::new(
        Arc::new(db.database.clone()),
        Arc::clone(&fake) as Arc<dyn BotApiSink>,
        Arc::new(DeliveryLimiter::new(30, 0)),
        Arc::clone(&clock) as Arc<dyn Clock>,
        limits(),
    );

    assert!(sender.run_once().await.expect("ambiguous send attempt"));
    assert_eq!(
        state(&db.database, job).await,
        (
            "outcome_unknown".to_owned(),
            Some("transport_unknown".to_owned())
        )
    );
    clock.advance_secs(120);
    assert!(!sender.run_once().await.expect("ordinary recovery scan"));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn known_ack_retries_local_recording_without_a_second_wire_call() {
    let db = database().await;
    let job = enqueue(&db.database, 575, "known acknowledgement").await;
    let fake = RecordingBotApi::new(Vec::new());
    let store = Arc::new(OneShotFaultAcknowledgementStore {
        database: db.database.clone(),
        calls: AtomicUsize::new(0),
    });
    let sender = OutboundSender::new_with_acknowledgement_store(
        Arc::new(db.database.clone()),
        Arc::clone(&store) as Arc<dyn AcknowledgementStore>,
        Arc::clone(&fake) as Arc<dyn BotApiSink>,
        Arc::new(DeliveryLimiter::new(30, 0)),
        FakeClock::at(T0),
        limits(),
    );

    assert!(sender.run_once().await.expect("known acknowledgement"));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.calls.load(Ordering::SeqCst), 2);
    assert_eq!(state(&db.database, job).await.0, "sent");
}

#[tokio::test]
async fn explicit_recovery_cannot_regress_a_newer_operation_binding() {
    let db = database().await;
    let operation_id = uuid::Uuid::now_v7();
    db.database
        .ensure_operation_binding(BOT_ID, operation_id, 600)
        .await
        .expect("binding");
    db.database
        .record_send_acknowledged(BOT_ID, operation_id, 600, 2000, T0)
        .await
        .expect("newer message");
    db.database
        .advance_render(operation_id, 600, 8, T0)
        .await
        .expect("newer revision");
    let original = enqueue(&db.database, 601, "audit evidence").await;
    let job = enqueue(&db.database, 600, "stale revision seven").await;
    sqlx::query(
        "update telegram.outbound_jobs
         set state = 'outcome_unknown', last_error_class = 'transport_unknown' where id = $1",
    )
    .bind(original)
    .execute(db.pool())
    .await
    .expect("quarantined audit evidence");
    sqlx::query(
        "update telegram.outbound_jobs
         set operation_id = $2, revision = 7, recovery_of = $3 where id = $1",
    )
    .bind(job)
    .bind(operation_id)
    .bind(original)
    .execute(db.pool())
    .await
    .expect("recovery marker");
    let fake = RecordingBotApi::new(Vec::new());
    let sender = OutboundSender::new(
        Arc::new(db.database.clone()),
        Arc::clone(&fake) as Arc<dyn BotApiSink>,
        Arc::new(DeliveryLimiter::new(30, 0)),
        FakeClock::at(T0),
        limits(),
    );

    assert!(sender.run_once().await.expect("stale recovery guard"));
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert_eq!(state(&db.database, job).await.0, "superseded");
    let binding = db
        .database
        .find_binding(operation_id, 600)
        .await
        .expect("binding read")
        .expect("binding remains");
    assert_eq!(binding.message_id, Some(2000));
    assert_eq!(binding.last_rendered_revision, 8);
}
