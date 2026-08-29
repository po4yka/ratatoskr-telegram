//! GitHub repository URL recognition and truthful Telegram rendering.

use ratatoskr_github_contracts::{
    ConfirmationEvidenceRef, GitHubRepositoryUrl, RepositoryActionAggregate,
    RepositoryActionCapability, RepositoryActionFailureReason, RepositoryActionIdempotencyKey,
    RepositoryActionRefusalReason, RepositoryActionRequest, RepositoryActionResult,
    RepositoryActionSkipReason, RepositoryDesiredBackupOutcome, RepositoryMetadataOutcome,
    RepositoryPreviewResponse, RepositoryProviderStarOutcome,
};
use telegram_persistence::dialogues::{CallbackRefusal, DecisionTransition, ReleasingUpdate};
use telegram_telemetry::metrics::{
    TELEGRAM_DIALOGUE_TRANSITIONS_TOTAL, TELEGRAM_INTERACTION_TOKEN_PRESENTATIONS_TOTAL,
};

use super::worker::CaptureContext;

const DIALOGUE_TTL_SECS: i64 = 15 * 60;
const ATTEMPTS: usize = 2;

/// Durable worker disposition after processing one repository callback.
pub(super) enum CallbackOutcome {
    /// The callback and any result projection committed.
    Processed,
    /// The callback is permanently unusable or internally invalid.
    Failed,
    /// Confirmation released work whose external or local result remains uncertain.
    Recoverable,
}

/// Parse exactly one canonical GitHub repository URL.
pub(super) fn parse_repository_url(text: &str) -> Option<GitHubRepositoryUrl> {
    let trimmed = text.trim();
    if trimmed.starts_with("/summarize") || trimmed.split_whitespace().count() != 1 {
        return None;
    }
    let parsed = url::Url::parse(trimmed).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<_> = parsed
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    let [owner, repository] = segments.as_slice() else {
        return None;
    };
    if std::path::Path::new(repository)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("git"))
    {
        return None;
    }
    GitHubRepositoryUrl::parse(&format!("https://github.com/{owner}/{repository}")).ok()
}

/// Escape untrusted provider text for Telegram HTML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render one read-only repository preview without inventing absent optional fields.
pub(super) fn render_preview(preview: &RepositoryPreviewResponse) -> String {
    let mut lines = vec![
        "<b>GitHub repository</b>".to_owned(),
        escape(preview.target.repository_full_name.as_str()),
    ];
    if let Some(description) = &preview.description {
        lines.push(escape(description.as_str()));
    }
    lines.push(format!("Stars: {}", preview.stargazer_count));
    if let Some(language) = &preview.primary_language {
        lines.push(format!("Language: {}", escape(language.as_str())));
    }
    lines.join("\n")
}

/// Stable display label for one action mode.
pub(super) const fn mode_label(mode: RepositoryActionCapability) -> &'static str {
    match mode {
        RepositoryActionCapability::Metadata => "metadata only",
        RepositoryActionCapability::Track => "track and request backup",
        RepositoryActionCapability::Star => "star on GitHub, catalog, and request backup",
        _ => "unsupported action",
    }
}

fn refusal(reason: RepositoryActionRefusalReason) -> &'static str {
    match reason {
        RepositoryActionRefusalReason::NotAuthorized => "refused: not authorized",
        RepositoryActionRefusalReason::AccountRequired => "refused: connected account required",
        RepositoryActionRefusalReason::AccountSelectionRequired => {
            "refused: account selection required"
        }
        RepositoryActionRefusalReason::ScopeMissing => "refused: required scope missing",
        RepositoryActionRefusalReason::TargetChanged => "refused: repository target changed",
        _ => "refused: unsupported reason",
    }
}

fn failure(reason: RepositoryActionFailureReason) -> &'static str {
    match reason {
        RepositoryActionFailureReason::DependencyUnavailable => "failed: dependency unavailable",
        RepositoryActionFailureReason::ProviderUnavailable => "failed: provider unavailable",
        RepositoryActionFailureReason::OutcomeUnknown => "failed: outcome unknown",
        RepositoryActionFailureReason::CatalogPersistenceFailed => "failed: catalog persistence",
        RepositoryActionFailureReason::PolicyPublicationFailed => {
            "failed: desired-policy publication"
        }
        _ => "failed: unsupported reason",
    }
}

fn skipped(reason: RepositoryActionSkipReason) -> &'static str {
    match reason {
        RepositoryActionSkipReason::NotApplicable => "skipped: not applicable",
        RepositoryActionSkipReason::PrerequisiteFailed => "skipped: prerequisite failed",
        _ => "skipped: unsupported reason",
    }
}

fn metadata(outcome: &RepositoryMetadataOutcome) -> &'static str {
    match outcome {
        RepositoryMetadataOutcome::Succeeded => "succeeded",
        RepositoryMetadataOutcome::AlreadyApplied => "already applied",
        RepositoryMetadataOutcome::Refused { reason } => refusal(*reason),
        RepositoryMetadataOutcome::Failed { reason } => failure(*reason),
        RepositoryMetadataOutcome::Skipped { reason } => skipped(*reason),
        _ => "unsupported outcome",
    }
}

fn star(outcome: &RepositoryProviderStarOutcome) -> &'static str {
    match outcome {
        RepositoryProviderStarOutcome::Succeeded => "succeeded",
        RepositoryProviderStarOutcome::AlreadyApplied => "already applied",
        RepositoryProviderStarOutcome::Refused { reason } => refusal(*reason),
        RepositoryProviderStarOutcome::Failed { reason } => failure(*reason),
        RepositoryProviderStarOutcome::Skipped { reason } => skipped(*reason),
        _ => "unsupported outcome",
    }
}

fn backup(outcome: &RepositoryDesiredBackupOutcome) -> &'static str {
    match outcome {
        RepositoryDesiredBackupOutcome::Accepted => "desired policy accepted (not yet verified)",
        RepositoryDesiredBackupOutcome::AlreadyApplied => {
            "desired policy already accepted (not verification)"
        }
        RepositoryDesiredBackupOutcome::Refused { reason } => refusal(*reason),
        RepositoryDesiredBackupOutcome::Failed { reason } => failure(*reason),
        RepositoryDesiredBackupOutcome::Skipped { reason } => skipped(*reason),
        _ => "unsupported outcome",
    }
}

/// Render GitHub's aggregate and every component without upgrading policy acceptance to backup.
pub(super) fn render_result(result: &RepositoryActionResult) -> String {
    let title = match result.aggregate {
        RepositoryActionAggregate::Succeeded => "Repository action result",
        RepositoryActionAggregate::Partial => "Repository action partially completed",
        RepositoryActionAggregate::Failed => "Repository action failed",
        _ => "Repository action result unavailable",
    };
    format!(
        "<b>{title}</b>\nMetadata: {}\nGitHub star: {}\nDesired backup: {}",
        metadata(&result.metadata),
        star(&result.provider_star),
        backup(&result.desired_backup),
    )
}

fn keyboard(buttons: impl IntoIterator<Item = (String, String)>) -> serde_json::Value {
    let rows: Vec<_> = buttons
        .into_iter()
        .map(|(text, callback_data)| serde_json::json!([{"text": text, "callback_data": callback_data}]))
        .collect();
    serde_json::json!({"inline_keyboard": rows})
}

fn record_callback_token(outcome: &'static str) {
    metrics::counter!(
        TELEGRAM_INTERACTION_TOKEN_PRESENTATIONS_TOTAL,
        "surface" => "callback",
        "outcome" => outcome,
    )
    .increment(1);
}

fn record_dialogue_transition(outcome: &'static str) {
    metrics::counter!(
        TELEGRAM_DIALOGUE_TRANSITIONS_TOTAL,
        "kind" => "github_repository",
        "outcome" => outcome,
    )
    .increment(1);
}

const fn callback_refusal_name(refusal: CallbackRefusal) -> &'static str {
    match refusal {
        CallbackRefusal::Invalid => "invalid",
        CallbackRefusal::Expired => "expired",
        CallbackRefusal::Consumed => "consumed",
    }
}

async fn enqueue(
    database: &telegram_persistence::Database,
    bot_id: i64,
    chat_id: i64,
    text: String,
    reply_markup: Option<serde_json::Value>,
    dialogue_id: Option<uuid::Uuid>,
    now: i64,
) -> Result<(), telegram_persistence::PersistenceError> {
    let job = message_job(bot_id, chat_id, text, reply_markup, dialogue_id)?;
    database.enqueue_outbound_job(&job, now).await?;
    Ok(())
}

fn message_job(
    bot_id: i64,
    chat_id: i64,
    text: String,
    reply_markup: Option<serde_json::Value>,
    dialogue_id: Option<uuid::Uuid>,
) -> Result<
    telegram_persistence::outbound_jobs::NewOutboundJob,
    telegram_persistence::PersistenceError,
> {
    let payload = telegram_persistence::outbound_jobs::MessagePayload {
        text,
        parse_mode: Some("HTML".to_owned()),
        reply_markup,
    };
    let content_hash = payload.canonical()?;
    Ok(telegram_persistence::outbound_jobs::NewOutboundJob {
        bot_id,
        chat_id,
        kind: telegram_persistence::outbound_jobs::OutboundJobKind::SendMessage,
        payload,
        content_hash,
        operation_id: None,
        revision: None,
        correlation_id: dialogue_id.map(|id| format!("telegram-dialogue:{id}")),
        next_attempt_at: None,
    })
}

fn transient(error: &platform_api::PlatformError) -> bool {
    matches!(
        error,
        platform_api::PlatformError::Network(_)
            | platform_api::PlatformError::Timeout
            | platform_api::PlatformError::RateLimited
            | platform_api::PlatformError::ServerError { .. }
    )
}

fn recoverable_action_error(error: &platform_api::PlatformError) -> bool {
    transient(error) || matches!(error, platform_api::PlatformError::Json(_))
}

const fn platform_failure_class(error: &platform_api::PlatformError) -> &'static str {
    match error {
        platform_api::PlatformError::Unauthenticated => "unauthenticated",
        platform_api::PlatformError::NotFound => "not_found",
        platform_api::PlatformError::Conflict => "conflict",
        platform_api::PlatformError::ClientError { .. } => "client_error",
        platform_api::PlatformError::MalformedFrame => "malformed_frame",
        platform_api::PlatformError::Network(_) => "network",
        platform_api::PlatformError::Timeout => "timeout",
        platform_api::PlatformError::RateLimited => "rate_limited",
        platform_api::PlatformError::ServerError { .. } => "server_error",
        platform_api::PlatformError::Json(_) => "invalid_response",
    }
}

async fn preview_with_retry(
    context: &CaptureContext,
    session: &str,
    request: &platform_api::RepositoryPreviewRequest,
) -> Result<RepositoryPreviewResponse, platform_api::PlatformError> {
    let mut last = None;
    for _ in 0..ATTEMPTS {
        match context
            .sessions
            .client()
            .preview_repository(session, request)
            .await
        {
            Ok(value) => return Ok(value),
            Err(error) if transient(&error) => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or(platform_api::PlatformError::Timeout))
}

/// Resolve and enqueue one read-only repository preview. No action endpoint is called here.
pub(super) async fn preview(
    database: &telegram_persistence::Database,
    bot_id: i64,
    chat_id: i64,
    actor_id: i64,
    repository_url: GitHubRepositoryUrl,
    context: &CaptureContext,
    now: i64,
) -> bool {
    let Ok(session) = context.sessions.credential(&actor_id.to_string()).await else {
        return enqueue(
            database,
            bot_id,
            chat_id,
            "<b>Repository preview unavailable</b>\nTry again later.".to_owned(),
            None,
            None,
            now,
        )
        .await
        .is_ok();
    };
    let request = platform_api::RepositoryPreviewRequest { repository_url };
    let Ok(preview) = preview_with_retry(context, &session, &request).await else {
        return enqueue(
            database,
            bot_id,
            chat_id,
            "<b>Repository preview unavailable</b>\nTry again later.".to_owned(),
            None,
            None,
            now,
        )
        .await
        .is_ok();
    };
    let Ok(dialogue) = database
        .create_repository_preview_dialogue(
            bot_id,
            actor_id,
            chat_id,
            &preview,
            now,
            now + DIALOGUE_TTL_SECS,
        )
        .await
    else {
        return false;
    };
    let buttons = dialogue
        .selections
        .into_iter()
        .map(|selection| (mode_label(selection.mode).to_owned(), selection.token));
    enqueue(
        database,
        bot_id,
        chat_id,
        render_preview(&preview),
        Some(keyboard(buttons)),
        Some(dialogue.dialogue_id),
        now,
    )
    .await
    .is_ok()
}

fn callback_message(callback: &bot_api::CallbackQuery) -> Option<(i64, i64, i64, &str)> {
    let message = callback.regular_message()?;
    Some((
        i64::try_from(callback.from.id.0).ok()?,
        message.chat.id.0,
        i64::from(message.id.0),
        callback.data.as_deref()?,
    ))
}

async fn answer(context: &CaptureContext, callback: &bot_api::CallbackQuery) {
    if let Err(error) = context.bot_api.answer_callback_query(&callback.id.0).await {
        tracing::warn!(class = "callback_answer_failed", error = %error, "callback answer failed");
    }
}

async fn selection(
    database: &telegram_persistence::Database,
    bot_id: i64,
    actor: i64,
    chat: i64,
    message: i64,
    opaque: &str,
    now: i64,
) -> Result<Option<bool>, telegram_persistence::PersistenceError> {
    match database
        .consume_repository_selection(opaque, bot_id, actor, chat, message, now)
        .await?
    {
        Ok(next) => {
            record_callback_token("released");
            record_dialogue_transition("preview_to_confirming");
            let text = format!(
                "<b>Confirm repository action</b>\n{}\nNothing is written until you confirm.",
                mode_label(next.mode)
            );
            let markup = keyboard([
                ("Confirm".to_owned(), next.confirm_token),
                ("Cancel".to_owned(), next.cancel_token),
            ]);
            enqueue(
                database,
                bot_id,
                chat,
                text,
                Some(markup),
                Some(next.dialogue_id),
                now,
            )
            .await?;
            Ok(Some(true))
        }
        Err(CallbackRefusal::Invalid | CallbackRefusal::Consumed) => Ok(None),
        Err(refusal) => {
            record_callback_token(callback_refusal_name(refusal));
            record_dialogue_transition(if refusal == CallbackRefusal::Expired {
                "expired"
            } else {
                "refused"
            });
            enqueue(
                database,
                bot_id,
                chat,
                "This action has expired. Please start again.".to_owned(),
                None,
                None,
                now,
            )
            .await?;
            Ok(Some(true))
        }
    }
}

async fn action_with_retry(
    context: &CaptureContext,
    session: &str,
    request: &RepositoryActionRequest,
) -> Result<RepositoryActionResult, platform_api::PlatformError> {
    let mut last = None;
    for _ in 0..ATTEMPTS {
        match context
            .sessions
            .client()
            .apply_repository_action(session, request)
            .await
        {
            Ok(value) => return Ok(value),
            Err(error) if transient(&error) => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or(platform_api::PlatformError::Timeout))
}

async fn finish_action_error(
    database: &telegram_persistence::Database,
    dialogue_id: uuid::Uuid,
    releasing_update: ReleasingUpdate,
    chat_id: i64,
    error: platform_api::PlatformError,
    now: i64,
) -> Result<CallbackOutcome, telegram_persistence::PersistenceError> {
    if recoverable_action_error(&error) {
        return Ok(CallbackOutcome::Recoverable);
    }
    tracing::warn!(
        class = platform_failure_class(&error),
        "Platform permanently refused the confirmed repository action",
    );
    let job = message_job(
        releasing_update.bot_id,
        chat_id,
        "<b>Repository action could not be accepted</b>\nNo success is claimed. Start again."
            .to_owned(),
        None,
        Some(dialogue_id),
    )?;
    database
        .complete_repository_dialogue_with_failure_job(dialogue_id, releasing_update, &job, now)
        .await?;
    record_dialogue_transition("completed_refusal");
    Ok(CallbackOutcome::Processed)
}

async fn execute_confirmed_action(
    database: &telegram_persistence::Database,
    action: telegram_persistence::dialogues::ConfirmedAction,
    releasing_update: ReleasingUpdate,
    actor: i64,
    chat: i64,
    context: &CaptureContext,
    now: i64,
) -> Result<CallbackOutcome, telegram_persistence::PersistenceError> {
    record_dialogue_transition("confirming_to_submitting");
    let Ok(evidence) =
        ConfirmationEvidenceRef::parse(&format!("telegram-confirmation:{}", action.dialogue_id))
    else {
        return Ok(CallbackOutcome::Failed);
    };
    let Ok(key) = RepositoryActionIdempotencyKey::parse(&action.idempotency_key) else {
        return Ok(CallbackOutcome::Failed);
    };
    let dialogue_id = action.dialogue_id;
    let Ok(request) = RepositoryActionRequest::new(
        action.mode,
        action.target,
        action.account_ref,
        evidence,
        key,
    ) else {
        return Ok(CallbackOutcome::Failed);
    };
    let session = match context.sessions.credential(&actor.to_string()).await {
        Ok(session) => session,
        Err(error) => {
            return finish_action_error(database, dialogue_id, releasing_update, chat, error, now)
                .await;
        }
    };
    let result = match action_with_retry(context, &session, &request).await {
        Ok(result) => result,
        Err(error) => {
            return finish_action_error(database, dialogue_id, releasing_update, chat, error, now)
                .await;
        }
    };
    let job = message_job(
        releasing_update.bot_id,
        chat,
        render_result(&result),
        None,
        Some(dialogue_id),
    )?;
    database
        .complete_repository_dialogue_with_result_job(
            dialogue_id,
            releasing_update,
            &result,
            &job,
            now,
        )
        .await?;
    record_dialogue_transition("completed");
    Ok(CallbackOutcome::Processed)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the closed callback presentation is intentionally checked field-by-field"
)]
async fn decision(
    database: &telegram_persistence::Database,
    bot_id: i64,
    update_id: i64,
    actor: i64,
    chat: i64,
    message: i64,
    opaque: &str,
    context: &CaptureContext,
    now: i64,
) -> Result<CallbackOutcome, telegram_persistence::PersistenceError> {
    let transition = database
        .consume_repository_decision(
            opaque,
            ReleasingUpdate { bot_id, update_id },
            actor,
            chat,
            message,
            now,
        )
        .await?;
    let transition = match transition {
        Ok(transition) => {
            record_callback_token("released");
            transition
        }
        Err(refusal) => {
            record_callback_token(callback_refusal_name(refusal));
            record_dialogue_transition(if refusal == CallbackRefusal::Expired {
                "expired"
            } else {
                "refused"
            });
            enqueue(
                database,
                bot_id,
                chat,
                "This action has expired. Please start again.".to_owned(),
                None,
                None,
                now,
            )
            .await?;
            return Ok(CallbackOutcome::Processed);
        }
    };
    if transition == DecisionTransition::AlreadyCompleted {
        return Ok(CallbackOutcome::Processed);
    }
    let DecisionTransition::Confirmed(action) = transition else {
        record_dialogue_transition("cancelled");
        enqueue(
            database,
            bot_id,
            chat,
            "Repository action cancelled. No write was sent.".to_owned(),
            None,
            None,
            now,
        )
        .await?;
        return Ok(CallbackOutcome::Processed);
    };
    execute_confirmed_action(
        database,
        action,
        ReleasingUpdate { bot_id, update_id },
        actor,
        chat,
        context,
        now,
    )
    .await
}

/// Process one recognized callback. Selection only prompts; only a confirmed token can submit.
pub(super) async fn handle_callback(
    database: &telegram_persistence::Database,
    bot_id: i64,
    update_id: i64,
    callback: &bot_api::CallbackQuery,
    context: &CaptureContext,
    now: i64,
) -> CallbackOutcome {
    let Some((actor, chat, message, opaque)) = callback_message(callback) else {
        answer(context, callback).await;
        return CallbackOutcome::Failed;
    };
    let selected = selection(database, bot_id, actor, chat, message, opaque, now).await;
    answer(context, callback).await;
    match selected {
        Ok(Some(true)) => CallbackOutcome::Processed,
        Ok(Some(false)) | Err(_) => CallbackOutcome::Failed,
        Ok(None) => decision(
            database, bot_id, update_id, actor, chat, message, opaque, context, now,
        )
        .await
        .unwrap_or(CallbackOutcome::Recoverable),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_repository_url, render_preview, render_result};

    #[test]
    fn only_exact_repository_urls_route_to_preview() {
        assert!(parse_repository_url("https://github.com/owner/repository").is_some());
        assert!(parse_repository_url("https://github.com/owner/repository/").is_some());
        for rejected in [
            "http://github.com/owner/repository",
            "https://github.com/owner/repository.git",
            "https://github.com/owner/repository/issues",
            "https://github.com/owner/repository?q=1",
            "https://github.com/owner/repository#readme",
            "/summarize https://github.com/owner/repository",
        ] {
            assert!(
                parse_repository_url(rejected).is_none(),
                "accepted {rejected}"
            );
        }
    }

    #[test]
    fn preview_rendering_escapes_provider_text_and_omits_missing_fields() {
        let preview = serde_json::from_value(serde_json::json!({
            "target": {"github_repository_numeric_id": 42, "repository_full_name": "owner/repository", "canonical_url": "https://github.com/owner/repository"},
            "description": "A <tool>", "stargazer_count": 42, "primary_language": "Rust",
            "available_actions": ["metadata", "track", "star"]
        })).expect("preview contract");
        assert_eq!(
            render_preview(&preview),
            "<b>GitHub repository</b>\nowner/repository\nA &lt;tool&gt;\nStars: 42\nLanguage: Rust"
        );

        let absent = serde_json::from_value(serde_json::json!({
            "target": {"github_repository_numeric_id": 42, "repository_full_name": "owner/repository", "canonical_url": "https://github.com/owner/repository"},
            "stargazer_count": 42, "available_actions": ["metadata"]
        })).expect("preview contract");
        let rendered = render_preview(&absent);
        assert!(!rendered.contains("Description:"));
        assert!(!rendered.contains("Language:"));
    }

    #[test]
    fn partial_result_renders_each_component_without_backup_or_atomic_success_claim() {
        let result = serde_json::from_value(serde_json::json!({
            "aggregate": "partial",
            "metadata": {"status": "succeeded"},
            "provider_star": {"status": "succeeded"},
            "desired_backup": {"status": "failed", "reason": "policy_publication_failed"}
        }))
        .expect("result contract");
        let rendered = render_result(&result);
        assert!(rendered.contains("partially completed"));
        assert!(rendered.contains("Metadata: succeeded"));
        assert!(rendered.contains("GitHub star: succeeded"));
        assert!(rendered.contains("Desired backup: failed: desired-policy publication"));
        assert!(!rendered.contains("backup completed"));
        assert!(!rendered.contains("backup verified"));
    }
}
