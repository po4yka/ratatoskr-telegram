//! The processing worker: the asynchronous half of admission.
//!
//! One task uses the bounded queue as a wake-up hint and claims work from `PostgreSQL`. For every
//! accepted update it walks the row through
//! `processing` to exactly one terminal state — `processed` for kinds this build acts on,
//! `unsupported` for kinds it does not — and logs settlement failures with their class rather
//! than swallowing them. Plan items 3+ replace the body of [`process_one`]; the intake contract
//! around it does not move.
//!
//! The task is detached by design: after the shutdown grace window closes, queued-but-unprocessed
//! items remain processable in the database rather than silently gone.

use telegram_persistence::{Database, UpdateState};
use tracing::Instrument as _;

use crate::intake::QueuedUpdate;
use crate::intake::classify::supported;

/// Drain the queue forever. Spawned once per process; aborted only by process exit.
pub async fn run_worker(
    database: Database,
    mut receiver: tokio::sync::mpsc::Receiver<QueuedUpdate>,
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
/// Errors are logged with their class and leave the row in its last honest state — never silently
/// swallowed, never retried inline. A retry belongs to whoever reprocesses `accepted`/`failed`
/// rows, which is the durable-queue work of a later item.
pub async fn process_one(database: &Database, item: &QueuedUpdate) {
    let span = tracing::info_span!(
        "telegram.update.process",
        update_id = item.update.id.0,
        bot_id = item.bot_id,
    );

    async {
        let terminal = if supported(&item.update.kind) {
            UpdateState::Processed
        } else {
            UpdateState::Unsupported
        };

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
