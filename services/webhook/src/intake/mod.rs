//! Update admission: verify the secret, enforce the limits, parse the schema, deduplicate, hand
//! off, acknowledge — in that order and no other.
//!
//! The one POST route is the whole public surface. Every rejection class is typed
//! ([`Outcome`]) and counted by safe class; a malformed payload is ACKED rather than rejected so
//! Telegram's retry machinery never turns one bad body into a storm. Downstream work happens only
//! through the bounded queue the handler hands to, never inline.

mod admit;
mod build;
mod classify;
mod worker;

pub use crate::intake::build::build;

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::extract::State;
use axum::routing::any;
use secrecy::SecretString;
use telegram_persistence::Database;
use telegram_telemetry::metrics::{
    TELEGRAM_WEBHOOK_DURATION_SECONDS, TELEGRAM_WEBHOOK_REQUESTS_TOTAL,
};

pub use crate::intake::admit::{QueuedUpdate, read_body_capped};
pub use crate::intake::classify::{kind_label, supported};
pub use crate::intake::worker::{process_one, run_worker};

/// Queue capacity. Bounded on purpose: a burst larger than this answers 503 and Telegram retries,
/// instead of the process buying memory it did not budget.
pub const QUEUE_CAPACITY: usize = 1024;

/// The path Telegram delivers updates to, relative to the configured public origin.
const WEBHOOK_PATH: &str = "/webhook";

/// Everything one admission decision reads.
#[derive(Debug, Clone)]
pub struct IntakeSettings {
    /// The configured webhook secret; compared constant-time against the delivery header.
    pub secret: SecretString,
    /// The request-body ceiling in bytes.
    pub max_body_bytes: usize,
    /// This bot's user id from `getMe`; half of every deduplication key.
    pub bot_id: i64,
    /// Queue capacity, overridable so tests can saturate honestly.
    pub queue_capacity: usize,
}

/// The intake state: settings, the database the dedupe row goes through, the queue sender.
pub struct Intake {
    /// Immutable admission inputs.
    pub settings: IntakeSettings,
    /// The pool update deduplication writes through.
    pub database: Database,
    /// The bounded handoff into asynchronous processing.
    pub sender: tokio::sync::mpsc::Sender<QueuedUpdate>,
}

impl std::fmt::Debug for Intake {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Settings carry the secret; render only its presence and the harmless numbers.
        formatter
            .debug_struct("Intake")
            .field("secret", &"[REDACTED]")
            .field("max_body_bytes", &self.settings.max_body_bytes)
            .field("bot_id", &self.settings.bot_id)
            .finish_non_exhaustive()
    }
}

impl Intake {
    /// Build the intake state and take the receiving end of its queue.
    ///
    /// The caller owns the receiver: production spawns [`run_worker`] with it, tests hold it to
    /// prove that acknowledgment does not wait for processing.
    #[must_use]
    pub fn new(
        settings: IntakeSettings,
        database: Database,
    ) -> (Arc<Self>, tokio::sync::mpsc::Receiver<QueuedUpdate>) {
        let capacity = if settings.queue_capacity == 0 {
            QUEUE_CAPACITY
        } else {
            settings.queue_capacity
        };
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity);
        let intake = Arc::new(Self {
            settings,
            database,
            sender,
        });
        (intake, receiver)
    }

    /// The public router. One route; admission order lives in [`admit`].
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route(WEBHOOK_PATH, any(admit))
            .with_state(Arc::clone(self))
    }
}

/// How one delivered request ended. Closed vocabulary — these strings are metric labels, so a
/// request must never be able to invent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Admitted for processing.
    Accepted,
    /// A redelivery; answered 200 and dropped by exact-match dedupe.
    Deduplicated,
    /// Missing or wrong secret header.
    Unauthorized,
    /// Body above the configured ceiling.
    TooLarge,
    /// Content type other than application/json.
    WrongMediaType,
    /// Anything but POST.
    MethodNotAllowed,
    /// Unparseable envelope: acked 200, logged, nothing recorded.
    Malformed,
    /// Queue saturated or storage failed: 503, Telegram retries, no side effect.
    Overloaded,
}

impl Outcome {
    /// The label value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Deduplicated => "deduplicated",
            Self::Unauthorized => "unauthorized",
            Self::TooLarge => "too_large",
            Self::WrongMediaType => "wrong_media_type",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::Malformed => "malformed",
            Self::Overloaded => "overloaded",
        }
    }

    /// The HTTP status Telegram receives.
    #[must_use]
    pub const fn status(self) -> http::StatusCode {
        use http::StatusCode;
        match self {
            Self::Accepted | Self::Deduplicated | Self::Malformed => StatusCode::OK,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::WrongMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Overloaded => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// Whether the outcome leaves evidence that must be logged at warning or above.
    #[must_use]
    pub const fn is_rejection(self) -> bool {
        !matches!(self, Self::Accepted | Self::Deduplicated | Self::Malformed)
    }
}

/// The route handler: times admission, applies the order, counts the outcome.
///
/// Deliberately thin — the ordering rules live in [`admit::admit_ordered`] so they are one function
/// a reviewer can read top to bottom against AGENTS.md's webhook-security list.
async fn admit(
    State(intake): State<Arc<Intake>>,
    request: http::Request<axum::body::Body>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    let started = Instant::now();
    let (parts, body) = request.into_parts();
    let outcome = admit::admit_ordered(&intake, &parts, body).await;

    metrics::counter!(TELEGRAM_WEBHOOK_REQUESTS_TOTAL, "outcome" => outcome.as_str()).increment(1);
    histogram_record(started.elapsed());

    if outcome.is_rejection() {
        tracing::warn!(
            outcome = outcome.as_str(),
            status = %outcome.status(),
            duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "webhook request admitted with a rejection outcome",
        );
    }

    (outcome.status(), admit::response_body(outcome)).into_response()
}

fn histogram_record(duration: std::time::Duration) {
    metrics::histogram!(TELEGRAM_WEBHOOK_DURATION_SECONDS).record(duration.as_secs_f64());
}
