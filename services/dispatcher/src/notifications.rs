//! Notification contract parsing, privacy-minimal rendering, and fixed `JetStream` consumption.

use std::path::Path;
use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt as _;
use metrics::{counter, gauge};
use ratatoskr_event_envelope::EventEnvelope;
use ratatoskr_notification_contracts::{NotificationPriority, NotificationRaised};
use telegram_core::config::NotificationBusConfig;
use telegram_persistence::notification_delivery::{
    NewNotificationDelivery, NotificationAdmissionResult, NotificationDecisionOutcome,
};
use telegram_persistence::{Database, PersistenceError};
use telegram_telemetry::metrics::{
    TELEGRAM_NOTIFICATION_BACKLOG, TELEGRAM_NOTIFICATION_EVENTS_TOTAL, TELEGRAM_NOTIFICATION_LAG,
};

/// Safe startup failure class with no endpoint, path, provider response, or credential detail.
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum NotificationBusError {
    /// The credentials file could not be read as NATS credentials.
    #[error("notification bus credentials are unavailable")]
    Credentials,
    /// NATS could not be reached or authenticated.
    #[error("notification bus connection is unavailable")]
    Connection,
    /// The fixed stream or durable cannot be inspected.
    #[error("notification durable is unavailable")]
    DurableUnavailable,
    /// Platform provisioned a consumer with a different contract.
    #[error("notification durable is incompatible")]
    DurableMismatch,
}

impl NotificationBusError {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Credentials => "credentials_unavailable",
            Self::Connection => "connection_unavailable",
            Self::DurableUnavailable => "durable_unavailable",
            Self::DurableMismatch => "durable_mismatch",
        }
    }
}

/// What `JetStream` must do after one database-backed admission attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportDisposition {
    /// Processing committed or was a harmless duplicate.
    Ack,
    /// Permanent malformed input was reduced to content-free evidence.
    Term,
    /// A transient database failure must be redelivered after a bounded pause.
    Nak,
}

fn safe_class(class: &str) -> &'static str {
    match class {
        "operation_completed" => "operation_completed",
        "operation_failed" => "operation_failed",
        "analysis_ready" => "analysis_ready",
        "backup_outcome" => "backup_outcome",
        "watch_triggered" => "watch_triggered",
        "archive_imported" => "archive_imported",
        _ => "other",
    }
}

fn record_outcome(outcome: &'static str, class: &str) {
    counter!(
        TELEGRAM_NOTIFICATION_EVENTS_TOTAL,
        "outcome" => outcome,
        "class" => safe_class(class),
    )
    .increment(1);
}

/// Record consumer backlog from provider-owned sequence metadata using identifier-free gauges.
pub fn record_consumer_progress(pending: u64) {
    let backlog = f64::from(u32::try_from(pending).unwrap_or(u32::MAX));
    let lag = f64::from(u32::try_from(pending.saturating_add(1)).unwrap_or(u32::MAX));
    gauge!(TELEGRAM_NOTIFICATION_BACKLOG).set(backlog);
    gauge!(TELEGRAM_NOTIFICATION_LAG).set(lag);
}

/// Record the sender settlement for a notification job. Direct and projection jobs have no
/// notification class and therefore do not enter the notification metric family.
pub fn record_delivery_outcome(
    notification_class: Option<&str>,
    attempts: i32,
    max_attempts: i32,
    outcome: &telegram_persistence::outbound_jobs::DeliveryOutcome,
) {
    let Some(class) = notification_class else {
        return;
    };
    let metric_outcome = match outcome {
        telegram_persistence::outbound_jobs::DeliveryOutcome::Sent
        | telegram_persistence::outbound_jobs::DeliveryOutcome::NotModified => "delivered",
        telegram_persistence::outbound_jobs::DeliveryOutcome::RetryWithBackoff { .. }
            if attempts >= max_attempts =>
        {
            "terminal"
        }
        telegram_persistence::outbound_jobs::DeliveryOutcome::RetryWithBackoff { .. } => "retry",
        telegram_persistence::outbound_jobs::DeliveryOutcome::FailedPermanent { .. }
        | telegram_persistence::outbound_jobs::DeliveryOutcome::SupersededStale
        | telegram_persistence::outbound_jobs::DeliveryOutcome::OutcomeUnknown { .. } => "terminal",
    };
    record_outcome(metric_outcome, class);
}

/// Render one validated notification into a privacy-minimal Telegram payload.
#[must_use]
pub fn render_notification(
    notification: &NotificationRaised,
) -> telegram_persistence::outbound_jobs::MessagePayload {
    let mut text = format!("<b>{}</b>", escape(notification.title.as_str()));
    if let Some(message) = notification.message.as_ref() {
        text.push('\n');
        text.push_str(&escape(message.as_str()));
    }
    telegram_persistence::outbound_jobs::MessagePayload {
        text,
        parse_mode: Some("HTML".to_owned()),
        reply_markup: None,
    }
}

/// Connect with least-privilege credentials and open, but never create, the fixed durable.
///
/// # Errors
///
/// A safe closed [`NotificationBusError`] for credentials, connection, lookup, or mismatch.
pub async fn open_preprovisioned_consumer(
    config: &NotificationBusConfig,
) -> Result<jetstream::consumer::PullConsumer, NotificationBusError> {
    let credentials = config
        .credentials_file
        .as_deref()
        .ok_or(NotificationBusError::Credentials)?;
    open_preprovisioned_consumer_at(config, credentials).await
}

async fn open_preprovisioned_consumer_at(
    config: &NotificationBusConfig,
    credentials: &Path,
) -> Result<jetstream::consumer::PullConsumer, NotificationBusError> {
    let seed =
        std::fs::read_to_string(credentials).map_err(|_| NotificationBusError::Credentials)?;
    let seed = seed.trim();
    if seed.is_empty() || seed.lines().count() != 1 {
        return Err(NotificationBusError::Credentials);
    }
    let options = async_nats::ConnectOptions::with_nkey(seed.to_owned());
    let client = options
        .connect(config.endpoint.as_str())
        .await
        .map_err(|_| NotificationBusError::Connection)?;
    let context = jetstream::new(client);
    lookup_preprovisioned_consumer(&context, config).await
}

/// Inspect and verify the fixed durable through an already connected `JetStream` context.
/// This helper performs no create/update/delete request.
///
/// # Errors
///
/// [`NotificationBusError::DurableUnavailable`] when it is absent/unreadable and
/// [`NotificationBusError::DurableMismatch`] when its cached config differs.
pub async fn lookup_preprovisioned_consumer(
    context: &jetstream::Context,
    config: &NotificationBusConfig,
) -> Result<jetstream::consumer::PullConsumer, NotificationBusError> {
    let consumer: jetstream::consumer::PullConsumer = context
        .get_consumer_from_stream(&config.durable, &config.stream)
        .await
        .map_err(|_| NotificationBusError::DurableUnavailable)?;
    verify_consumer(&consumer, config)?;
    Ok(consumer)
}

/// Verify the cached consumer contract without any `JetStream` mutation.
///
/// # Errors
///
/// [`NotificationBusError::DurableMismatch`] for any topology drift.
pub fn verify_consumer(
    consumer: &jetstream::consumer::PullConsumer,
    config: &NotificationBusConfig,
) -> Result<(), NotificationBusError> {
    let actual = &consumer.cached_info().config;
    if actual.durable_name.as_deref() != Some(config.durable.as_str())
        || actual.filter_subject != config.subject
        || actual.ack_policy != jetstream::consumer::AckPolicy::Explicit
        || actual.ack_wait != Duration::from_secs(config.ack_wait_seconds)
        || actual.deliver_subject.is_some()
        || actual.deliver_policy != jetstream::consumer::DeliverPolicy::All
        || actual.replay_policy != jetstream::consumer::ReplayPolicy::Instant
    {
        return Err(NotificationBusError::DurableMismatch);
    }
    Ok(())
}

/// Supervise initial connection and reopen the pre-provisioned durable after transport failure.
pub async fn supervise(
    config: NotificationBusConfig,
    database: Database,
    bot_id: i64,
    runtime: std::sync::Arc<telegram_http::RuntimeState>,
) {
    loop {
        match open_preprovisioned_consumer(&config).await {
            Ok(consumer) => {
                consume_until_interrupted(
                    consumer,
                    database.clone(),
                    bot_id,
                    config.fetch_batch,
                    std::sync::Arc::clone(&runtime),
                )
                .await;
            }
            Err(error) => tracing::warn!(
                class = error.as_str(),
                "notification dependency is not ready",
            ),
        }
        runtime.set_notification_reachable(false);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Run one verified consumer until its transport stream is interrupted. Database admission
/// commits before ack; the supervisor performs the next durable lookup.
async fn consume_until_interrupted(
    consumer: jetstream::consumer::PullConsumer,
    database: Database,
    bot_id: i64,
    fetch_batch: u32,
    runtime: std::sync::Arc<telegram_http::RuntimeState>,
) {
    let stream = consumer
        .stream()
        .max_messages_per_batch(usize::try_from(fetch_batch).unwrap_or(32))
        .messages()
        .await;
    let Ok(mut messages) = stream else {
        runtime.set_notification_reachable(false);
        return;
    };
    runtime.set_notification_reachable(true);
    while let Some(next) = messages.next().await {
        let Ok(message) = next else {
            runtime.set_notification_reachable(false);
            return;
        };
        let info = message.info().ok();
        if let Some(info) = info.as_ref() {
            record_consumer_progress(info.pending);
        }
        let sequence = info.as_ref().map(|info| info.stream_sequence);
        let disposition = process_payload(
            &database,
            bot_id,
            sequence,
            message.payload.as_ref(),
            jiff::Timestamp::now().as_second(),
        )
        .await;
        let ack = match disposition {
            TransportDisposition::Ack => jetstream::AckKind::Ack,
            TransportDisposition::Term => jetstream::AckKind::Term,
            TransportDisposition::Nak => jetstream::AckKind::Nak(Some(Duration::from_secs(2))),
        };
        if message.ack_with(ack).await.is_err() {
            runtime.set_notification_reachable(false);
            return;
        }
    }
    runtime.set_notification_reachable(false);
}

/// Parse and admit one payload with no transport side effect. The real loop and tests share it.
pub async fn process_payload(
    database: &Database,
    bot_id: i64,
    stream_sequence: Option<u64>,
    bytes: &[u8],
    now: i64,
) -> TransportDisposition {
    let Ok(envelope) = EventEnvelope::from_json(bytes) else {
        record_outcome("received", "other");
        return permanent_failure(database, stream_sequence, None, "invalid_envelope", now).await;
    };
    let event_id = envelope.event_id.0;
    let notification = match envelope.payload_as::<NotificationRaised>() {
        Ok(notification) => notification,
        Err(ratatoskr_event_envelope::EnvelopeError::PayloadType { .. }) => {
            record_outcome("received", "other");
            return permanent_failure(
                database,
                stream_sequence,
                Some(event_id),
                "wrong_event_type",
                now,
            )
            .await;
        }
        Err(_) => {
            record_outcome("received", "other");
            return permanent_failure(
                database,
                stream_sequence,
                Some(event_id),
                "invalid_notification",
                now,
            )
            .await;
        }
    };
    let metric_class = notification.class.as_str();
    record_outcome("received", metric_class);
    if envelope.tenant_id != Some(notification.recipient) {
        return permanent_failure(
            database,
            stream_sequence,
            Some(event_id),
            "invalid_notification",
            now,
        )
        .await;
    }
    let quiet_hint_seconds = notification
        .quiet_hours
        .map(|hint| (hint.start_offset_seconds(), hint.end_offset_seconds()));
    let delivery = NewNotificationDelivery {
        bot_id,
        event_id,
        stream_sequence,
        notification_id: notification.notification_id.0,
        recipient_user_id: notification.recipient.user_id().0,
        class: notification.class.as_str().to_owned(),
        priority_high: matches!(notification.priority_hint, Some(NotificationPriority::High)),
        quiet_hint_seconds,
        payload: render_notification(&notification),
        correlation_id: Some(envelope.correlation_id.to_wire()),
        occurred_at: envelope.occurred_at.as_jiff().as_second(),
    };
    match database.admit_notification(&delivery, now).await {
        Ok(
            NotificationAdmissionResult::DuplicateTransport
            | NotificationAdmissionResult::DuplicateNotification,
        ) => {
            record_outcome("duplicate", metric_class);
            TransportDisposition::Ack
        }
        Ok(NotificationAdmissionResult::NoEligibleChat) => {
            record_outcome("terminal", metric_class);
            TransportDisposition::Ack
        }
        Ok(NotificationAdmissionResult::Decided(outcomes)) => {
            for outcome in outcomes {
                let label = match outcome {
                    NotificationDecisionOutcome::Suppressed => "suppressed",
                    NotificationDecisionOutcome::Deferred => "deferred",
                    NotificationDecisionOutcome::Enqueued => "enqueued",
                };
                if outcome != NotificationDecisionOutcome::Suppressed {
                    record_outcome("enabled", metric_class);
                }
                record_outcome(label, metric_class);
            }
            TransportDisposition::Ack
        }
        Err(_) => {
            record_outcome("retry", metric_class);
            TransportDisposition::Nak
        }
    }
}

async fn permanent_failure(
    database: &Database,
    stream_sequence: Option<u64>,
    event_id: Option<uuid::Uuid>,
    class: &'static str,
    now: i64,
) -> TransportDisposition {
    match database
        .record_notification_transport_failure(stream_sequence, event_id, class, now)
        .await
    {
        Ok(()) => {
            record_outcome("terminal", "other");
            TransportDisposition::Term
        }
        Err(PersistenceError::Query(_) | PersistenceError::Connect(_)) => {
            record_outcome("retry", "other");
            TransportDisposition::Nak
        }
        Err(_) => TransportDisposition::Nak,
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
