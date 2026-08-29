//! Capture submission outcome mapping at the durable worker boundary.

use telegram_persistence::{Database, UpdateState};

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
            ProcessingOutcome::RetryAcceptedProjection
        }
        Err(class) => {
            metrics::counter!(
                telegram_telemetry::metrics::TELEGRAM_CAPTURE_SUBMISSIONS_TOTAL,
                "class" => class.as_str(),
            )
            .increment(1);
            tracing::warn!(class = class.as_str(), "the capture could not be submitted");
            UpdateState::Failed.into()
        }
    }
}
