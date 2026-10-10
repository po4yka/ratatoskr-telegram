//! Capture submission outcome mapping at the durable worker boundary.

use telegram_persistence::{Database, UpdateState};

use super::attachment::{ReplyKind, reply_and_fail};
use super::{CaptureContext, ProcessingOutcome};
use crate::intake::capture;

/// Submit one capture intent and map the outcome to the update's terminal state.
pub(crate) async fn run_capture(
    database: &Database,
    bot_id: i64,
    chat_id: i64,
    telegram_user_id: i64,
    source: platform_api::CaptureSource,
    context: &CaptureContext,
    metadata: Option<telegram_persistence::interaction_tokens::IntentMetadata>,
) -> ProcessingOutcome {
    match capture::submit(
        &context.sessions,
        database,
        bot_id,
        chat_id,
        telegram_user_id,
        source,
        metadata,
    )
    .await
    {
        Ok(accepted) => {
            tracing::info!(
                operation = %accepted.operation_id,
                "a capture was submitted and acknowledged",
            );
            UpdateState::Processed.into()
        }
        Err(capture::SubmitClass::AcceptedProjectionPending) => {
            metrics::counter!(
                telegram_telemetry::metrics::TELEGRAM_CAPTURE_SUBMISSIONS_TOTAL,
                "class" => capture::SubmitClass::AcceptedProjectionPending.as_str(),
            )
            .increment(1);
            ProcessingOutcome::RetryRecoverableAction
        }
        Err(class @ capture::SubmitClass::TransientExhausted) => {
            refuse(
                database,
                bot_id,
                chat_id,
                class,
                ReplyKind::CaptureUnavailable,
            )
            .await
        }
        Err(class @ capture::SubmitClass::PermanentRefusal) => {
            refuse(database, bot_id, chat_id, class, ReplyKind::CaptureRefused).await
        }
    }
}

/// Count, log, and settle a capture Platform did not accept: the sender hears about every such
/// failure (S11) through one static reply, then the update is `Failed` whether or not the reply
/// could be stored. Each class names its own reply at the call site, so a class that must not
/// reach this path cannot be given a misleading one.
async fn refuse(
    database: &Database,
    bot_id: i64,
    chat_id: i64,
    class: capture::SubmitClass,
    reply: ReplyKind,
) -> ProcessingOutcome {
    metrics::counter!(
        telegram_telemetry::metrics::TELEGRAM_CAPTURE_SUBMISSIONS_TOTAL,
        "class" => class.as_str(),
    )
    .increment(1);
    tracing::warn!(class = class.as_str(), "the capture could not be submitted");
    reply_and_fail(database, bot_id, chat_id, reply).await
}
