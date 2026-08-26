//! The projection consumer: render, hash, and hand one event to the transactional accept step.
//!
//! The consumer is deliberately thin — every guard lives in persistence's
//! `accept_operation_event`, where dedup, terminal, staleness, revision, sweep, and enqueue are
//! one transaction. What remains here is dispatcher policy: rendering the body and hashing it so
//! identical re-renders are detectable downstream.

use std::fmt::Write as _;
use std::sync::Arc;

use telegram_persistence::Database;
use telegram_persistence::PersistenceError;
use telegram_persistence::outbound_jobs::MessagePayload;
use telegram_persistence::projection_accept::{AcceptOutcome as PersistenceOutcome, AcceptedEvent};

use crate::outbound::clock::Clock;
use crate::projection::event::OperationEvent;
use crate::projection::render::render;

/// How one accepted event ended at the consumer boundary. Mirrors persistence's outcome without
/// leaking its revision payload; counting per outcome is phase E's telemetry task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptOutcome {
    /// The event was accepted and its edit enqueued.
    Recorded,
    /// This envelope was already consumed.
    Duplicate,
    /// The binding is already terminal.
    PostTerminal,
    /// The event is older than the newest accepted one.
    Stale,
    /// No binding exists for the operation; nothing was written.
    Unbound,
}

/// The consumer over one database, one clock, and the configured minimum render interval.
#[derive(Clone)]
pub struct ProjectionConsumer {
    /// The pool the accept transaction runs on.
    pub(crate) database: Database,
    /// The injected time source for `now` inside the accept step.
    pub(crate) clock: Arc<dyn Clock>,
    /// Minimum seconds between eligible edits of one binding (design D4).
    pub(crate) render_interval_secs: u64,
    /// The serving bot's username, when known; deep-link buttons need it.
    pub(crate) bot_username: Option<String>,
}

impl std::fmt::Debug for ProjectionConsumer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectionConsumer")
            .field("render_interval_secs", &self.render_interval_secs)
            .finish_non_exhaustive()
    }
}

impl ProjectionConsumer {
    /// Assemble a consumer. Configuration arrives typed from `DispatcherConfig` in a later phase;
    /// the raw interval is taken now so the seam does not move.
    #[must_use]
    pub fn new(
        database: Database,
        clock: Arc<dyn Clock>,
        render_interval_secs: u64,
        bot_username: Option<String>,
    ) -> Self {
        Self {
            database,
            clock,
            render_interval_secs,
            bot_username,
        }
    }

    /// Run one event through the guards: render, hash, and the transactional accept.
    ///
    /// # Errors
    ///
    /// [`PersistenceError`] when the accept transaction fails; a storage failure is never
    /// masqueraded as an outcome.
    pub async fn accept(&self, event: &OperationEvent) -> Result<AcceptOutcome, PersistenceError> {
        let body = render(event);
        let payload = if event.status.is_terminal() {
            let intent = self
                .database
                .find_live_intent_by_operation(event.operation_id, self.clock.now_secs())
                .await
                .ok()
                .flatten();
            super::compose::compose_terminal(
                body,
                event,
                intent.as_ref(),
                self.bot_username.as_deref(),
            )
        } else {
            MessagePayload::text(body)
        };
        let payload_json = payload.canonical()?;
        let content_hash = sha256_hex(&payload_json);
        let now = self.clock.now_secs();

        let outcome = self
            .database
            .accept_operation_event(
                AcceptedEvent {
                    operation_id: event.operation_id,
                    event_id: event.event_id,
                    occurred_at_secs: event.occurred_at_secs,
                    terminal: event.status.is_terminal(),
                    payload_json: &payload_json,
                    content_hash: &content_hash,
                    correlation_id: &event.correlation_id,
                },
                now,
                i64::try_from(self.render_interval_secs).unwrap_or(i64::MAX),
            )
            .await?;

        let mapped = match outcome {
            PersistenceOutcome::Recorded { .. } => AcceptOutcome::Recorded,
            PersistenceOutcome::Duplicate => AcceptOutcome::Duplicate,
            PersistenceOutcome::PostTerminal => AcceptOutcome::PostTerminal,
            PersistenceOutcome::Stale => AcceptOutcome::Stale,
            PersistenceOutcome::Unbound => AcceptOutcome::Unbound,
        };
        metrics::counter!(
            telegram_telemetry::metrics::TELEGRAM_PROJECTION_EVENTS_TOTAL,
            "outcome" => mapped.as_str(),
        )
        .increment(1);
        Ok(mapped)
    }
}

/// The lowercase snake label for logs and the projection-events metric.
impl AcceptOutcome {
    /// The closed label value; a delivery can never mint a new one.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::Duplicate => "duplicate",
            Self::PostTerminal => "post_terminal",
            Self::Stale => "stale",
            Self::Unbound => "unbound",
        }
    }
}

/// Lowercase hex sha256 of `text` — the job's identical-render no-op key.
fn sha256_hex(text: &str) -> String {
    use sha2::Digest as _;

    let digest = sha2::Sha256::digest(text.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
