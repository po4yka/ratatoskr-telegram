//! Structured outbound payloads reach the wire whole: text, parse mode, and keyboard pass the
//! sender seam unchanged, and the content hash covers markup so markup-only changes edit.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::Arc;

use common::{FakeClock, database};
use ratatoskr_telegram_dispatcher::outbound::DeliveryLimiter;
use ratatoskr_telegram_dispatcher::outbound::sender::{
    BotApiSink, OutboundSender, SendFuture, SenderLimits, SentMessage,
};
use telegram_persistence::outbound_jobs::{MessagePayload, NewOutboundJob, OutboundJobKind};
use telegram_persistence::test_support::TestDatabase;
use uuid::Uuid;

/// A fixed synthetic instant: whole seconds since the Unix epoch.
const T0: i64 = 1_800_000_000;
/// The one bot every synthetic job belongs to.
const BOT_ID: i64 = 700_100_200;

mod common;

/// A minimal recording sink: payloads in, immediate success out.
#[derive(Debug, Default)]
struct RecordingSink {
    payloads: std::sync::Mutex<Vec<MessagePayload>>,
}

impl BotApiSink for RecordingSink {
    fn send_message(&self, _chat_id: i64, payload: &MessagePayload) -> SendFuture<'_> {
        let payload = payload.clone();
        Box::pin(async move {
            self.payloads.lock().expect("records").push(payload);
            Ok(SentMessage { message_id: 1 })
        })
    }

    fn edit_message_text(
        &self,
        _chat_id: i64,
        _message_id: i64,
        payload: &MessagePayload,
    ) -> SendFuture<'_> {
        let payload = payload.clone();
        Box::pin(async move {
            self.payloads.lock().expect("records").push(payload);
            Ok(SentMessage { message_id: 1 })
        })
    }
}

fn make_sender(db: &TestDatabase, sink: Arc<RecordingSink>) -> OutboundSender {
    let sink: Arc<dyn BotApiSink> = sink;
    OutboundSender::new(
        Arc::new(db.database.clone()),
        sink,
        Arc::new(DeliveryLimiter::new(30, 0)),
        FakeClock::at(T0),
        SenderLimits {
            max_attempts: 3,
            backoff_base_secs: 1,
            backoff_cap_secs: 60,
            jitter_fraction_milli: 0,
            lease_ttl_secs: 60,
        },
    )
}

async fn enqueue(db: &TestDatabase, payload: &MessagePayload, hash: &str) -> Uuid {
    db.database
        .enqueue_outbound_job(
            &NewOutboundJob {
                bot_id: BOT_ID,
                chat_id: 900_700_600,
                kind: OutboundJobKind::SendMessage,
                payload: payload.clone(),
                content_hash: hash.to_owned(),
                operation_id: None,
                revision: None,
                correlation_id: None,
                next_attempt_at: Some(T0),
            },
            T0,
        )
        .await
        .expect("the payload job enqueues")
}

/// The sink observes exactly what was enqueued - text, parse mode, keyboard - and equal-text
/// payloads with different markup carry different hashes, so markup-only changes really edit.
#[tokio::test]
async fn structured_payloads_reach_the_sink_verbatim_and_hash_distinguishes_markup() {
    let db = database().await;
    let sink = Arc::new(RecordingSink::default());
    let sender = make_sender(&db, Arc::clone(&sink));

    let markup = serde_json::json!({
        "inline_keyboard": [[{"text": "Open", "url": "https://t.me/ratatoskr_test_bot"}]]
    });
    let with_markup = MessagePayload {
        text: "<b>Completed</b>".to_owned(),
        parse_mode: Some("HTML".to_owned()),
        reply_markup: Some(markup.clone()),
    };
    enqueue(
        &db,
        &with_markup,
        &with_markup.canonical().expect("canonical"),
    )
    .await;
    sender.run_once().await.expect("run");

    let recorded = sink.payloads.lock().expect("records");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].text, "<b>Completed</b>");
    assert_eq!(recorded[0].parse_mode.as_deref(), Some("HTML"));
    assert_eq!(recorded[0].reply_markup.as_ref(), Some(&markup));

    // Equal text with different presentation hashes differently: not an identical re-render.
    let plain = MessagePayload::text("<b>Completed</b>");
    assert_ne!(
        with_markup.canonical().expect("canonical"),
        plain.canonical().expect("canonical")
    );
}
