//! The capture domain action: one parsed intent becomes one idempotent Platform operation, one
//! pre-created binding, one deep-link intent record, and one acknowledgment send job.
//!
//! Everything here runs inside the webhook worker after the access gate — never in the admission
//! path. Platform failures are classified once (transient classes retry within a small bound,
//! everything else settles immediately) and an unaccepted capture sends nothing.

use crate::intake::intent;

/// How many attempts one capture gets against transient Platform classes.
const SUBMIT_ATTEMPTS: usize = 2;
/// How long a deep-link intent stays resolvable: thirty days, then it expires silently.
pub(crate) const INTENT_TTL_SECS: i64 = 30 * 24 * 3_600;
/// The parse mode label every composed body carries.
const HTML: &str = "HTML";

/// Why one capture could not be submitted. A closed safe vocabulary for telemetry; no variant
/// carries the URL or any identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubmitClass {
    /// Transient classes persisted through the whole attempt bound.
    TransientExhausted,
    /// Platform refused the credential or request outright.
    PermanentRefusal,
}

impl SubmitClass {
    /// The metric label value.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::TransientExhausted => "transient_exhausted",
            Self::PermanentRefusal => "permanent_refusal",
        }
    }
}

/// What one accepted capture produced.
pub(crate) struct AcceptedCapture {
    /// The Platform operation to follow.
    pub operation_id: uuid::Uuid,
}

/// Submit one capture intent on behalf of `telegram_user_id`, and on acceptance write the
/// binding, the intent record, and the acknowledgment job.
///
/// The binding is written BEFORE any progress can arrive so early frames are applied rather than
/// dropped unbound; the acknowledgment send stamps its message id only after the Bot API
/// acknowledges (the sender owns that write).
pub(crate) async fn submit(
    sessions: &platform_api::session::SessionSource,
    database: &telegram_persistence::Database,
    bot_id: i64,
    chat_id: i64,
    telegram_user_id: i64,
    url: &str,
) -> Result<AcceptedCapture, SubmitClass> {
    let subject = telegram_user_id.to_string();
    let session = sessions
        .credential(&subject)
        .await
        .map_err(|error| classify(&error))?;
    let client = sessions.client();

    // Resending a link whose operation is already tracked must not duplicate the message:
    // Platform replays the original operation, and an existing live binding means the chat
    // already holds the acknowledgment for it.
    let key = intent::capture_key(telegram_user_id, url);
    let accepted: platform_api::OperationAccepted = submit_with_retries(
        client,
        &session,
        &platform_api::CaptureSubmission {
            idempotency_key: key,
            source: platform_api::CaptureSource::Url(url.to_owned()),
            origin: None,
        },
    )
    .await?;
    let operation_id = accepted.operation_id;

    let existing = database
        .find_binding(operation_id, chat_id)
        .await
        .map_err(|_| SubmitClass::TransientExhausted)?;
    if existing.is_none() {
        database
            .ensure_operation_binding(bot_id, operation_id, chat_id)
            .await
            .map_err(|_| SubmitClass::TransientExhausted)?;
        let intent_id = uuid::Uuid::now_v7();
        database
            .insert_intent(
                &telegram_persistence::intents::NewIntent {
                    id: intent_id,
                    bot_id,
                    telegram_user_id,
                    chat_id,
                    operation_id,
                    source_url: Some(url.to_owned()),
                    metadata: None,
                    expires_at_secs: now_secs() + INTENT_TTL_SECS,
                },
                now_secs(),
            )
            .await
            .map_err(|_| SubmitClass::TransientExhausted)?;
        let payload = telegram_persistence::outbound_jobs::MessagePayload {
            text: ack_body(url),
            parse_mode: Some(HTML.to_owned()),
            reply_markup: None,
        };
        let content_hash = payload
            .canonical()
            .map_err(|_| SubmitClass::TransientExhausted)?;
        database
            .enqueue_outbound_job(
                &telegram_persistence::outbound_jobs::NewOutboundJob {
                    bot_id,
                    chat_id,
                    kind: telegram_persistence::outbound_jobs::OutboundJobKind::SendMessage,
                    payload,
                    content_hash,
                    operation_id: Some(operation_id),
                    revision: None,
                    correlation_id: Some(format!("operation:{operation_id}")),
                    next_attempt_at: None,
                },
                now_secs(),
            )
            .await
            .map_err(|_| SubmitClass::TransientExhausted)?;
    }

    Ok(AcceptedCapture { operation_id })
}

/// One submission with the transient-class retry bound around it.
async fn submit_with_retries(
    client: &platform_api::Client,
    session: &str,
    submission: &platform_api::CaptureSubmission,
) -> Result<platform_api::OperationAccepted, SubmitClass> {
    let mut last = SubmitClass::TransientExhausted;
    for attempt in 0..SUBMIT_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        match client.submit_capture(session, submission).await {
            Ok(accepted) => return Ok(accepted),
            Err(error) if is_transient(&error) => last = SubmitClass::TransientExhausted,
            Err(_) => return Err(SubmitClass::PermanentRefusal),
        }
    }
    Err(last)
}

/// The closed split between "wait and try again" and "stop now".
fn is_transient(error: &platform_api::PlatformError) -> bool {
    matches!(
        error,
        platform_api::PlatformError::Network(_)
            | platform_api::PlatformError::Timeout
            | platform_api::PlatformError::ServerError { .. }
            | platform_api::PlatformError::RateLimited
    )
}

fn classify(error: &platform_api::PlatformError) -> SubmitClass {
    if is_transient(error) {
        SubmitClass::TransientExhausted
    } else {
        SubmitClass::PermanentRefusal
    }
}

fn now_secs() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// The acknowledgment body: the status lead, then the captured address as the fallback link.
fn ack_body(url: &str) -> String {
    format!(
        "<b>Capturing</b>\n<a href=\"{}\">{}</a>",
        escape(url),
        escape(url)
    )
}

/// Escape the five characters Telegram's HTML parse mode treats specially.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
