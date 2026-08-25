//! The processing worker: the asynchronous half of admission.
//!
//! One task uses the bounded queue as a wake-up hint and claims work from `PostgreSQL`. For every
//! accepted update it walks the row through
//! `processing` to exactly one terminal state — `processed` for kinds this build acts on,
//! `unsupported` for kinds it does not, `denied` when the authorization gate refuses the sender
//! or chat — and logs settlement failures with their class rather than swallowing them. Later
//! plan items replace the body of [`process_one`]; the intake contract around it does not move.
//!
//! The task is detached by design: after the shutdown grace window closes, queued-but-unprocessed
//! items remain processable in the database rather than silently gone.

use std::sync::Arc;

use telegram_persistence::{Database, UpdateState};
use telegram_telemetry::metrics::TELEGRAM_UPDATES_DENIED_TOTAL;
use tracing::Instrument as _;

use crate::intake::QueuedUpdate;
use crate::intake::access;
use crate::intake::capture;
use crate::intake::classify::supported;
use crate::intake::intent;

/// Everything the capture arm needs, built once at startup and shared across claims.
#[derive(Clone)]
pub struct CaptureContext {
    sessions: Arc<platform_api::session::SessionSource>,
}

impl std::fmt::Debug for CaptureContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CaptureContext")
            .finish_non_exhaustive()
    }
}

impl CaptureContext {
    /// Wire a context over an authenticated Platform session source.
    #[must_use]
    pub fn new(sessions: Arc<platform_api::session::SessionSource>) -> Self {
        Self { sessions }
    }
}

/// Drain the queue forever. Spawned once per process; aborted only by process exit.
pub async fn run_worker(
    database: Database,
    mut receiver: tokio::sync::mpsc::Receiver<QueuedUpdate>,
    capture_context: Option<CaptureContext>,
) {
    loop {
        match database.claim_update().await {
            Ok(Some(pending)) => match serde_json::from_str(&pending.payload) {
                Ok(update) => {
                    process_one(
                        &database,
                        &QueuedUpdate {
                            bot_id: pending.bot_id,
                            update,
                        },
                        capture_context.as_ref(),
                    )
                    .await;
                }
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        class = "stored_payload_invalid",
                        update_id = pending.update_id,
                        bot_id = pending.bot_id,
                        "a durable update payload could not be parsed",
                    );
                    let _ = database
                        .settle_update(pending.bot_id, pending.update_id, UpdateState::Failed)
                        .await;
                }
            },
            Ok(None) => {
                tokio::select! {
                    item = receiver.recv() => {
                        if item.is_none() {
                            break;
                        }
                    }
                    () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                }
            }
            Err(error) => {
                tracing::error!(error = %error, class = "claim_failed", "pending updates could not be claimed");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

/// Settle one queued update: `processing`, then its terminal state.
///
/// `capture` carries the Platform half of the domain action; admission-contract tests drive
/// processing with `None`, which keeps the pre-item-5 behavior of settling supported updates as
/// processed without acting. Production always wires a context.
///
/// Errors are logged with their class and leave the row in its last honest state — never silently
/// swallowed, never retried inline. A retry belongs to whoever reprocesses `accepted`/`failed`
/// rows, which is the durable-queue work of a later item.
pub async fn process_one(
    database: &Database,
    item: &QueuedUpdate,
    capture_context: Option<&CaptureContext>,
) {
    let span = tracing::info_span!(
        "telegram.update.process",
        update_id = item.update.id.0,
        bot_id = item.bot_id,
    );

    async {
        if let Err(error) = database
            .settle_update(
                item.bot_id,
                i64::from(item.update.id.0),
                UpdateState::Processing,
            )
            .await
        {
            tracing::error!(
                error = %error,
                class = "settlement_failed",
                "the update could not enter processing",
            );
            return;
        }

        // The gate runs between the two settlement writes: a refusal is an ordinary terminal
        // transition from here, and an unreadable policy is recorded as a failure rather than
        // improvised into a verdict.
        let terminal = if supported(&item.update.kind) {
            match access::authorize(database, &item.update).await {
                Ok(None) => self_domain_action(database, item, capture_context).await,
                Ok(Some(denial)) => {
                    metrics::counter!(TELEGRAM_UPDATES_DENIED_TOTAL, "class" => denial.as_str())
                        .increment(1);
                    // Class and correlation ids only — never the sender, the chat, or content
                    // (design D6): the three classes are externally indistinguishable.
                    tracing::info!(
                        class = denial.as_str(),
                        "the access policy refused the sender or chat",
                    );
                    UpdateState::Denied
                }
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        class = "authorization_check_failed",
                        "the access policy could not be evaluated",
                    );
                    UpdateState::Failed
                }
            }
        } else {
            UpdateState::Unsupported
        };

        match database
            .settle_update(item.bot_id, i64::from(item.update.id.0), terminal)
            .await
        {
            Ok(()) => {
                tracing::debug!(terminal = terminal.as_str(), "the update settled");
            }
            Err(error) => {
                tracing::error!(
                    error = %error,
                    class = "settlement_failed",
                    "the update could not settle",
                );
                // Leave `processing` evidence behind; a `failed` row is written only when the
                // database can record it.
                let _ = database
                    .settle_update(
                        item.bot_id,
                        i64::from(item.update.id.0),
                        UpdateState::Failed,
                    )
                    .await;
            }
        }
    }
    .instrument(span)
    .await;
}

/// The authorized-update arm: parse an intent, act on it, and answer with one terminal state.
///
/// A parsed URL or `/summarize` submits a capture; text without one is unsupported, silently as
/// every other kind this build does not act on. With no capture context wired (admission tests
/// only) a supported update keeps settling processed without acting.
async fn self_domain_action(
    database: &Database,
    item: &QueuedUpdate,
    capture_context: Option<&CaptureContext>,
) -> UpdateState {
    let Some(parts) = message_parts(&item.update.kind) else {
        return UpdateState::Processed;
    };
    let Some(intent) = parts.text.and_then(intent::parse) else {
        return UpdateState::Unsupported;
    };
    let Some(context) = capture_context else {
        // Test-only arm: no Platform half wired, nothing to act on.
        return UpdateState::Processed;
    };

    match capture::submit(
        &context.sessions,
        database,
        item.bot_id,
        parts.chat_id,
        parts.sender_id,
        &intent.url,
    )
    .await
    {
        Ok(accepted) => {
            tracing::info!(
                operation = %accepted.operation_id,
                "a capture was submitted and acknowledged",
            );
            UpdateState::Processed
        }
        Err(class) => {
            metrics::counter!(
                "telegram_capture_submissions_total",
                "class" => class.as_str(),
            )
            .increment(1);
            tracing::warn!(class = class.as_str(), "the capture could not be submitted");
            UpdateState::Failed
        }
    }
}

/// The pieces of a message update the domain action reads: its text, sender, and chat.
struct MessageParts<'a> {
    text: Option<&'a str>,
    sender_id: i64,
    chat_id: i64,
}

fn message_parts(kind: &bot_api::UpdateKind) -> Option<MessageParts<'_>> {
    let message = match kind {
        bot_api::UpdateKind::Message(message) | bot_api::UpdateKind::EditedMessage(message) => {
            message
        }
        _ => return None,
    };
    let sender = message.from.as_ref()?;
    Some(MessageParts {
        text: message.text(),
        sender_id: i64::try_from(sender.id.0).ok()?,
        chat_id: message.chat.id.0,
    })
}
