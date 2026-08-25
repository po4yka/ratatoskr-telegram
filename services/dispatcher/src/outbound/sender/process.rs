//! The per-job pipeline: supersede guard, limiter gate, wire call, classification, settlement.
//!
//! Order is load-bearing (design D3/D5): the supersede guard runs BEFORE any wire call so a stale
//! edit never reaches Telegram; the limiter gates BEFORE the wire so local budgets are honored;
//! binding writes happen only AFTER an acknowledged response, never from an attempt in flight.

use tracing::Instrument as _;

use telegram_persistence::outbound_jobs::{DeliveryOutcome, OutboundJobKind, QueuedOutboundJob};

use crate::outbound::classify::{Classified, PermanentClass, classify};
use crate::outbound::limiter::RateDecision;
use crate::outbound::sender::{OutboundSender, SenderError};

/// Which wire write one resolved job performs.
#[derive(Debug, Clone, Copy)]
enum WireAction {
    /// A fresh send.
    Send,
    /// An edit of an already-bound message.
    Edit {
        /// The bound Telegram message id to rewrite.
        message_id: i64,
    },
}

impl OutboundSender {
    /// Process one claimed job end to end, under a span that names the job without any chat or
    /// content fields.
    pub(super) async fn process(&self, job: QueuedOutboundJob) -> Result<(), SenderError> {
        let span = tracing::info_span!(
            "telegram.outbound.job",
            job_id = %job.id,
            kind = job.kind.as_str(),
            attempt = job.attempts,
        );
        self.process_inner(job).instrument(span).await
    }

    async fn process_inner(&self, job: QueuedOutboundJob) -> Result<(), SenderError> {
        match job.kind {
            OutboundJobKind::SendMessage => self.deliver_and_settle(&job, WireAction::Send).await,
            OutboundJobKind::EditMessageText => self.process_edit(job).await,
        }
    }

    /// Resolve an edit job against its binding before anything touches the wire.
    ///
    /// Guards, in order: an edit must name an operation and a revision (anything else cannot be
    /// ordered and is withdrawn); the binding must exist (the consumer edits only into existing
    /// bindings — design D7 — so absence means withdrawn, never an unsolicited send); the
    /// revision must be strictly newer than the last rendered one; and the bound message must
    /// still exist. That last case splits: a stale revision is withdrawn, while an UNBOUND but
    /// newer revision downgrades to a fresh send and rebinds on ack — the §19 fallback after a
    /// permanent failure cleared the message id.
    async fn process_edit(&self, job: QueuedOutboundJob) -> Result<(), SenderError> {
        let Some(operation_id) = job.operation_id else {
            tracing::warn!(
                class = "edit_without_operation",
                "an edit job names no operation; ordering is unknowable",
            );
            return self.settle(&job, DeliveryOutcome::SupersededStale).await;
        };
        let Some(revision) = job.revision else {
            tracing::debug!(
                class = "edit_without_revision",
                "an edit job carries no projection revision",
            );
            return self.settle(&job, DeliveryOutcome::SupersededStale).await;
        };
        let Some(binding) = self
            .database
            .find_binding(operation_id, job.chat_id)
            .await?
        else {
            tracing::debug!(
                class = "edit_without_binding",
                "no binding names this operation and chat",
            );
            return self.settle(&job, DeliveryOutcome::SupersededStale).await;
        };
        if binding.last_rendered_revision >= revision {
            return self.settle(&job, DeliveryOutcome::SupersededStale).await;
        }
        if let Some(message_id) = binding.message_id {
            self.deliver_and_settle(&job, WireAction::Edit { message_id })
                .await
        } else {
            tracing::debug!(
                class = "edit_downgraded_to_send",
                "the bound message is gone; this revision sends fresh",
            );
            self.deliver_and_settle(&job, WireAction::Send).await
        }
    }

    /// Gate, deliver, classify, settle — one pass over one resolved job.
    async fn deliver_and_settle(
        &self,
        job: &QueuedOutboundJob,
        action: WireAction,
    ) -> Result<(), SenderError> {
        match self.limiter.try_acquire(self.clock.as_ref(), job.chat_id) {
            RateDecision::Proceed => {}
            RateDecision::ChatWait { after_ms } | RateDecision::GlobalWait { after_ms } => {
                // The reschedule pushes next_attempt_at forward, so the row cannot be re-claimed
                // in a hot loop while the gate is closed.
                let delay_secs = u32::try_from(after_ms.div_ceil(1000)).unwrap_or(u32::MAX);
                return self
                    .settle(job, DeliveryOutcome::RetryWithBackoff { delay_secs })
                    .await;
            }
        }

        let now = self.clock.now_secs();
        // The histogram covers the wire call only: claim, guards, and settlement are queue work,
        // not delivery latency.
        let started = std::time::Instant::now();
        let result = match action {
            WireAction::Send => self.sink.send_message(job.chat_id, &job.payload).await,
            WireAction::Edit { message_id } => {
                self.sink
                    .edit_message_text(job.chat_id, message_id, &job.payload)
                    .await
            }
        };
        metrics::histogram!(telegram_telemetry::metrics::TELEGRAM_DELIVERY_DURATION_SECONDS)
            .record(started.elapsed().as_secs_f64());

        match result {
            Ok(sent) => {
                self.apply_success(job, action, sent.message_id, now)
                    .await?;
                self.settle(job, DeliveryOutcome::Sent).await
            }
            Err(error) => self.apply_failure(job, classify(&error), now).await,
        }
    }

    /// Binding effects of an acknowledged delivery. Provider message ids are written only here —
    /// after the Bot API said yes, never from an attempt still in flight.
    async fn apply_success(
        &self,
        job: &QueuedOutboundJob,
        action: WireAction,
        message_id: i64,
        now: i64,
    ) -> Result<(), SenderError> {
        let Some(operation_id) = job.operation_id else {
            // A generic send names no operation; it creates no binding traffic at all.
            return Ok(());
        };
        match (job.kind, action) {
            // A fresh send establishes or rebinds the binding with the returned id.
            (OutboundJobKind::SendMessage, _)
            | (OutboundJobKind::EditMessageText, WireAction::Send) => {
                self.database
                    .ensure_operation_binding(job.bot_id, operation_id, job.chat_id)
                    .await?;
                self.database
                    .record_send_acknowledged(
                        job.bot_id,
                        operation_id,
                        job.chat_id,
                        message_id,
                        now,
                    )
                    .await?;
            }
            // A normal edit keeps its message id; only the render state moves below.
            (OutboundJobKind::EditMessageText, WireAction::Edit { .. }) => {}
        }
        if let Some(revision) = job.revision {
            // `false` means a newer render already won the race; the Sent settlement stands and
            // the stale advance is dropped as harmless.
            let _ = self
                .database
                .advance_render(operation_id, job.chat_id, revision, now)
                .await?;
        }
        Ok(())
    }

    /// Settlement per outcome class (design D5's action column).
    async fn apply_failure(
        &self,
        job: &QueuedOutboundJob,
        decision: Classified,
        now: i64,
    ) -> Result<(), SenderError> {
        match decision {
            Classified::NotModified => {
                self.settle(job, DeliveryOutcome::NotModified).await?;
                if let (Some(operation_id), Some(revision)) = (job.operation_id, job.revision) {
                    // Identical bytes are still a successful render of this revision.
                    let _ = self
                        .database
                        .advance_render(operation_id, job.chat_id, revision, now)
                        .await?;
                }
                Ok(())
            }
            Classified::RateLimited { retry_after_secs } => {
                tracing::info!(
                    class = "rate_limited",
                    retry_after_secs,
                    "telegram asked the sender for a pause",
                );
                metrics::counter!(telegram_telemetry::metrics::TELEGRAM_RATE_LIMIT_WAITS_TOTAL)
                    .increment(1);
                metrics::counter!(
                    telegram_telemetry::metrics::TELEGRAM_DELIVERY_RETRIES_TOTAL,
                    "class" => "rate_limited",
                )
                .increment(1);
                self.limiter
                    .penalize(job.chat_id, now.saturating_add(retry_after_secs));
                let pause = u32::try_from(retry_after_secs.max(0)).unwrap_or(u32::MAX);
                let delay_secs = pause.saturating_add(self.jitter_secs(pause));
                self.settle(job, DeliveryOutcome::RetryWithBackoff { delay_secs })
                    .await
            }
            Classified::Transient | Classified::Sent => {
                // `Sent` is unreachable from `classify`; a bounded retry beats inventing a
                // settlement for a variant that should never arrive.
                let exhausted = i64::from(job.attempts) >= i64::from(self.limits.max_attempts);
                if exhausted {
                    // Persistence dead-letters this settlement; the sender knows it first.
                    metrics::counter!(
                        telegram_telemetry::metrics::TELEGRAM_DELIVERY_FAILURES_TOTAL,
                        "class" => "dead_letter",
                    )
                    .increment(1);
                } else {
                    metrics::counter!(
                        telegram_telemetry::metrics::TELEGRAM_DELIVERY_RETRIES_TOTAL,
                        "class" => "transient",
                    )
                    .increment(1);
                }
                let delay_secs = self.transient_backoff_secs(job.attempts);
                self.settle(job, DeliveryOutcome::RetryWithBackoff { delay_secs })
                    .await
            }
            Classified::Permanent { class } => {
                metrics::counter!(
                    telegram_telemetry::metrics::TELEGRAM_DELIVERY_FAILURES_TOTAL,
                    "class" => class.as_str(),
                )
                .increment(1);
                self.settle(
                    job,
                    DeliveryOutcome::FailedPermanent {
                        class: class.as_str().to_owned(),
                    },
                )
                .await?;
                if let (OutboundJobKind::EditMessageText, Some(operation_id)) =
                    (job.kind, job.operation_id)
                    && matches!(
                        class,
                        PermanentClass::BotBlocked
                            | PermanentClass::ChatNotFound
                            | PermanentClass::MembershipLost
                            | PermanentClass::MessageNotEditable
                            | PermanentClass::EditTargetGone
                    )
                {
                    // The bound message is unrecoverable for edits, so the id is cleared and
                    // the next revision sends fresh and rebinds. Blocked/not-found chats also
                    // poison sends: a subsequent SEND surfaces the same permanent class and
                    // stops there, rather than the queue spinning on edits of a dead target.
                    self.database
                        .unbind_message(operation_id, job.chat_id, now)
                        .await?;
                }
                Ok(())
            }
        }
    }

    /// Settle one claimed job through persistence, which owns the dead-letter-at-bound rule.
    async fn settle(
        &self,
        job: &QueuedOutboundJob,
        outcome: DeliveryOutcome,
    ) -> Result<(), SenderError> {
        let max_attempts = i32::try_from(self.limits.max_attempts).unwrap_or(i32::MAX);
        self.database
            .settle_outbound_job(job.id, self.clock.now_secs(), max_attempts, &outcome)
            .await?;
        Ok(())
    }

    /// Capped exponential backoff for the Nth attempt (attempts counted at claim), plus jitter.
    fn transient_backoff_secs(&self, attempts: i32) -> u32 {
        let exponent = u32::try_from(attempts.saturating_sub(1))
            .unwrap_or(0)
            .min(30);
        let grown = self
            .limits
            .backoff_base_secs
            .saturating_mul(2u32.saturating_pow(exponent))
            .min(self.limits.backoff_cap_secs);
        grown.saturating_add(self.jitter_secs(grown))
    }

    /// The random share of a delay: `jitter_fraction_milli` of `value_secs`, drawn through the
    /// injected clock so tests stay deterministic.
    fn jitter_secs(&self, value_secs: u32) -> u32 {
        let bound_ms =
            u64::from(value_secs).saturating_mul(u64::from(self.limits.jitter_fraction_milli));
        u32::try_from(self.clock.jitter_millis(bound_ms) / 1000).unwrap_or(0)
    }
}
