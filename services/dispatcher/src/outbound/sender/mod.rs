//! The outbound sender: claim one due job, guard it, gate it, deliver it, settle it.
//!
//! The loop body is deliberately claim-ONE-then-process: every property the delivery suite pins
//! (per-chat FIFO, one job in flight, supersede-before-the-wire) holds per claim, so concurrency
//! is a scheduling concern of [`OutboundSender::run_forever`] and never changes outcomes.
//!
//! One cost is accepted and documented here rather than hidden: attempts increment AT CLAIM
//! (persistence's crash-honesty decision), so a limiter deferral after a claim consumes an
//! attempt slot. It is rare by construction — claim eligibility already filtered on
//! `next_attempt_at`, so a deferral only happens when the local rate gates close between enqueue
//! and claim — and the deferral also pushes `next_attempt_at` forward, so the row cannot be
//! re-claimed in a hot loop while it waits.

mod process;
mod sink;

pub use crate::outbound::sender::sink::{BotApiSink, ClientSink, SendFuture, SentMessage};

use std::sync::Arc;
use std::time::Duration;

use telegram_persistence::Database;

use crate::outbound::clock::Clock;
use crate::outbound::limiter::DeliveryLimiter;

/// The sender's tuning knobs, one struct so wiring stays a single argument.
#[derive(Debug, Clone, Copy)]
pub struct SenderLimits {
    /// Claims before a job dead-letters; enforced by persistence at settlement.
    pub max_attempts: u32,
    /// First transient backoff, seconds.
    pub backoff_base_secs: u32,
    /// Transient backoff ceiling, seconds.
    pub backoff_cap_secs: u32,
    /// Jitter fraction in thousandths (`100` = 10%): the upper bound of the random share added
    /// to a computed delay.
    pub jitter_fraction_milli: u32,
    /// How long a claim's lease runs before a crashed sender's orphan is reclaimable.
    pub lease_ttl_secs: u32,
}

/// Why one sender pass ended abnormally. Sink failures are classified into settlements, not
/// propagated; only persistence failures — which leave the row honestly `sending` until its lease
/// expires — reach the caller.
#[derive(Debug, thiserror::Error)]
pub enum SenderError {
    /// A claim or settlement statement failed.
    #[error("an outbound job could not be claimed or settled")]
    Persistence(#[from] telegram_persistence::PersistenceError),
}

/// The durable delivery worker over one database and one Bot API seam.
#[derive(Clone)]
pub struct OutboundSender {
    /// The pool the queue, bindings, and settlements go through.
    pub(crate) database: Arc<Database>,
    /// The wire seam calls go out through.
    pub(crate) sink: Arc<dyn BotApiSink>,
    /// The shared rate gate; penalties from `429`s land here too.
    pub(crate) limiter: Arc<DeliveryLimiter>,
    /// The injected time source for eligibility, backoff, and render stamps.
    pub(crate) clock: Arc<dyn Clock>,
    /// Tuning knobs.
    pub(crate) limits: SenderLimits,
}

impl std::fmt::Debug for OutboundSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The sink and clock are trait objects without Debug; render only their presence plus
        // the harmless handles and numbers, mirroring how the webhook intake redacts its secret.
        formatter
            .debug_struct("OutboundSender")
            .field("database", &self.database)
            .field("limiter", &self.limiter)
            .field("sink", &"dyn BotApiSink")
            .field("clock", &"dyn Clock")
            .field("limits", &self.limits)
            .finish()
    }
}

impl OutboundSender {
    /// Assemble a sender over shared handles.
    #[must_use]
    pub fn new(
        database: Arc<Database>,
        sink: Arc<dyn BotApiSink>,
        limiter: Arc<DeliveryLimiter>,
        clock: Arc<dyn Clock>,
        limits: SenderLimits,
    ) -> Self {
        Self {
            database,
            sink,
            limiter,
            clock,
            limits,
        }
    }

    /// Claim and fully process ONE due job. Returns whether it did work.
    ///
    /// # Errors
    ///
    /// [`SenderError::Persistence`] when claiming or settling fails; the claimed row (if any)
    /// stays `sending` until its lease expires and is then reclaimed, so no work is lost.
    pub async fn run_once(&self) -> Result<bool, SenderError> {
        let now = self.clock.now_secs();
        let Some(job) = self
            .database
            .claim_due_outbound_job(now, self.limits.lease_ttl_secs)
            .await?
        else {
            return Ok(false);
        };
        self.process(job).await?;
        Ok(true)
    }

    /// Drain the queue forever, spawned once per process.
    ///
    /// `wake` is a hint channel: receiving an item retries immediately instead of waiting out the
    /// idle poll, and a CLOSED channel (every sender dropped) ends the loop — shutdown owns the
    /// senders, so closing them is the stop signal. Mirrors the webhook worker's shape.
    pub async fn run_forever(self, mut wake: tokio::sync::mpsc::Receiver<()>) {
        loop {
            match self.run_once().await {
                Ok(true) => {}
                Ok(false) => {
                    tokio::select! {
                        item = wake.recv() => {
                            if item.is_none() {
                                break;
                            }
                        }
                        () = tokio::time::sleep(Duration::from_secs(1)) => {}
                    }
                }
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        class = "claim_failed",
                        "due outbound jobs could not be claimed",
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
}
