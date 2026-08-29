//! Dispatcher shutdown: cancellation closes admission while an admitted delivery reaches its
//! durable outcome boundary.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::routing::post;
use ratatoskr_telegram_dispatcher::outbound::DeliveryLimiter;
use ratatoskr_telegram_dispatcher::outbound::sender::{
    BotApiSink, OutboundSender, SendFuture, SenderLimits, SentMessage,
};
use telegram_persistence::outbound_jobs::{MessagePayload, NewOutboundJob, OutboundJobKind};
use telegram_persistence::test_support::TestDatabase;
use uuid::Uuid;

use common::{FakeClock, database};

const NOW: i64 = 1_800_000_000;
const BOT_ID: i64 = 700_100_200;

#[derive(Debug)]
struct Gate {
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

#[derive(Debug)]
struct GatedBotApi {
    first: Mutex<Option<Gate>>,
}

impl GatedBotApi {
    fn new(gate: Gate) -> Arc<Self> {
        Arc::new(Self {
            first: Mutex::new(Some(gate)),
        })
    }

    fn deliver(&self) -> SendFuture<'_> {
        Box::pin(async move {
            let gate = self.first.lock().expect("gate").take();
            if let Some(gate) = gate {
                let _ = gate.started.send(());
                let _ = gate.release.await;
            }
            Ok(SentMessage { message_id: 101 })
        })
    }
}

impl BotApiSink for GatedBotApi {
    fn send_message(&self, _chat_id: i64, _payload: &MessagePayload) -> SendFuture<'_> {
        self.deliver()
    }

    fn edit_message_text(
        &self,
        _chat_id: i64,
        _message_id: i64,
        _payload: &MessagePayload,
    ) -> SendFuture<'_> {
        self.deliver()
    }
}

async fn enqueue(db: &TestDatabase, chat_id: i64, body: &str) -> Uuid {
    db.database
        .enqueue_outbound_job(
            &NewOutboundJob {
                bot_id: BOT_ID,
                chat_id,
                kind: OutboundJobKind::SendMessage,
                payload: MessagePayload::text(body),
                content_hash: format!("hash-{body}"),
                operation_id: None,
                revision: None,
                correlation_id: None,
                next_attempt_at: Some(NOW),
            },
            NOW,
        )
        .await
        .expect("enqueue")
}

async fn state(db: &TestDatabase, id: Uuid) -> String {
    sqlx::query_scalar("select state from telegram.outbound_jobs where id = $1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .expect("job state")
}

/// Raising the sender's process-lifetime cancellation flag closes admission. The job already
/// inside its Bot API/settlement critical section must finish, but the loop must observe shutdown
/// before its next claim.
#[tokio::test]
async fn shutdown_stops_new_claims_and_waits_for_inflight_delivery() {
    let db = database().await;
    let first = enqueue(&db, 100, "admitted before shutdown").await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let sink = GatedBotApi::new(Gate {
        started: started_tx,
        release: release_rx,
    });
    let sender = OutboundSender::new(
        Arc::new(db.database.clone()),
        sink,
        Arc::new(DeliveryLimiter::new(30, 0)),
        FakeClock::at(NOW),
        SenderLimits {
            max_attempts: 5,
            backoff_base_secs: 1,
            backoff_cap_secs: 60,
            jitter_fraction_milli: 0,
            lease_ttl_secs: 30,
        },
    );
    let (_wake_tx, wake_rx) = tokio::sync::mpsc::channel::<()>(1);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(sender.run_until_shutdown(
        wake_rx,
        Duration::from_hours(1),
        shutdown_rx,
        Arc::new(tokio::sync::RwLock::new(())),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ));

    started_rx.await.expect("the first delivery starts");
    shutdown_tx.send_replace(true);
    let second = enqueue(&db, 200, "ready after shutdown").await;
    release_tx.send(()).expect("release first delivery");
    worker
        .await
        .expect("the sender joins after its admitted work");

    assert_eq!(state(&db, first).await, "sent", "in-flight work settles");
    assert_eq!(
        state(&db, second).await,
        "ready",
        "shutdown is observed before the next claim"
    );
}

/// The real runtime shutdown request seals admission synchronously, before its fence task is first
/// polled. A newly spawned sender therefore cannot pass its final check afterward.
#[tokio::test]
async fn shutdown_fence_wins_before_a_new_claim() {
    let db = database().await;
    let job = enqueue(&db, 300, "not admitted after shutdown").await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    drop(release_tx);
    let sink = GatedBotApi::new(Gate {
        started: started_tx,
        release: release_rx,
    });
    let sender = OutboundSender::new(
        Arc::new(db.database.clone()),
        sink,
        Arc::new(DeliveryLimiter::new(30, 0)),
        FakeClock::at(NOW),
        SenderLimits {
            max_attempts: 5,
            backoff_base_secs: 1,
            backoff_cap_secs: 60,
            jitter_fraction_milli: 0,
            lease_ttl_secs: 30,
        },
    );
    let (_wake_tx, wake_rx) = tokio::sync::mpsc::channel::<()>(1);
    let (cancel, shutdown_rx) = tokio::sync::watch::channel(false);
    let mut background = telegram_http::BackgroundRuntime::from_tasks(cancel, Vec::new());
    let admission = background.admission_fence();
    let admission_closed = background.admission_closed();
    background.spawn(sender.run_until_shutdown(
        wake_rx,
        Duration::from_hours(1),
        shutdown_rx,
        admission,
        admission_closed,
    ));
    background.request_shutdown();
    background.join().await;

    assert!(
        started_rx.await.is_err(),
        "the real shutdown request seals admission before returning"
    );
    assert_eq!(state(&db, job).await, "ready", "no post-shutdown claim");
}

#[tokio::test]
async fn production_runtime_owns_all_four_worker_roles() {
    let db = database().await;
    let app = axum::Router::new().route(
        "/bot123456:synthetic/GetMe",
        post(|| async {
            axum::Json(serde_json::json!({
                "ok": true,
                "result": {
                    "id": BOT_ID,
                    "is_bot": true,
                    "first_name": "Synthetic",
                    "username": "synthetic_bot",
                    "can_join_groups": true,
                    "can_read_all_group_messages": false,
                    "supports_inline_queries": false,
                    "can_connect_to_business": false,
                    "has_main_web_app": false
                }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Bot API");
    let address = listener.local_addr().expect("fake address");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config: telegram_core::TelegramConfig = serde_json::from_value(serde_json::json!({
        "admin": {"bind": "127.0.0.1:0"},
        "bot_api": {
            "base_url": format!("http://{address}"),
            "timeout_seconds": 5,
            "token": "123456:synthetic",
            "username": "synthetic_bot"
        },
        "platform": {
            "base_url": "http://127.0.0.1:9",
            "timeout_seconds": 1,
            "audience": "runtime-test",
            "assertion_signing_key": "00".repeat(32)
        },
        "shutdown": {"drain_seconds": 0, "grace_seconds": 1},
        "telemetry": {}
    }))
    .expect("synthetic config");
    let process_state = Arc::new(telegram_http::RuntimeState::new(
        telegram_core::RuntimeRole::Dispatcher,
    ));
    let mut background =
        ratatoskr_telegram_dispatcher::build::build(telegram_http::PublicContext {
            config: Arc::new(config),
            database: Some(db.database.clone()),
            runtime: process_state,
        })
        .await
        .expect("production composition builds");

    assert_eq!(
        background.task_count(),
        4,
        "sender, projection, follower, and notification are all lifecycle-owned"
    );
    background.request_shutdown();
    background.join().await;
    server.abort();
    let _ = server.await;
}
