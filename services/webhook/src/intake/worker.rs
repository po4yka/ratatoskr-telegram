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

use ratatoskr_identifiers::MediaType;
use ratatoskr_telegram_blob_store::BlobStore;
use telegram_persistence::{Database, UpdateState};
use telegram_telemetry::metrics::TELEGRAM_UPDATES_DENIED_TOTAL;
use tracing::Instrument as _;

use crate::intake::QueuedUpdate;
use crate::intake::access;
use crate::intake::capture;
use crate::intake::classify::supported;
use crate::intake::github;
use crate::intake::intent;

/// Everything the capture arm needs, built once at startup and shared across claims.
#[derive(Clone)]
pub struct CaptureContext {
    pub(super) sessions: Arc<platform_api::session::SessionSource>,
    pub(super) bot_api: bot_api::Client,
    blobs: BlobStore,
    max_attachment_bytes: u64,
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
    pub fn new(
        sessions: Arc<platform_api::session::SessionSource>,
        bot_api: bot_api::Client,
        blobs: BlobStore,
        max_attachment_bytes: u64,
    ) -> Self {
        Self {
            sessions,
            bot_api,
            blobs,
            max_attachment_bytes,
        }
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
/// A forwarded message loosens the grammar: its first http(s) link - in text or caption - becomes
/// a capture intent with the forward origin preserved as provenance, and a linkless forward is
/// unsupported. Non-forwarded text keeps the strict grammar: one bare URL or `/summarize <url>`;
/// anything else is unsupported, silently as every other kind this build does not act on. With no
/// capture context wired (admission tests only) a supported update keeps settling processed
/// without acting.
async fn self_domain_action(
    database: &Database,
    item: &QueuedUpdate,
    capture_context: Option<&CaptureContext>,
) -> UpdateState {
    if let bot_api::UpdateKind::CallbackQuery(callback) = &item.update.kind {
        let Some(context) = capture_context else {
            return UpdateState::Processed;
        };
        return if github::handle_callback(database, item.bot_id, callback, context, now_secs())
            .await
        {
            UpdateState::Processed
        } else {
            UpdateState::Failed
        };
    }
    let Some(parts) = message_parts(&item.update.kind) else {
        return UpdateState::Processed;
    };
    let Some(context) = capture_context else {
        // Test-only arm: no Platform half wired, nothing to act on.
        return UpdateState::Processed;
    };

    if parts.forward.is_some() {
        let link = parts.text.or(parts.caption).and_then(first_https_link);
        let metadata = parts.forward.clone().map(url_metadata);
        return match link {
            Some(url) => {
                run_capture(
                    database,
                    item.bot_id,
                    parts.chat_id,
                    parts.sender_id,
                    platform_api::CaptureSource::Url(url),
                    context,
                    metadata,
                )
                .await
            }
            None => handle_attachment(database, item.bot_id, &parts, context).await,
        };
    }

    if let Some(repository_url) = parts.text.and_then(github::parse_repository_url) {
        return if github::preview(
            database,
            item.bot_id,
            parts.chat_id,
            parts.sender_id,
            repository_url,
            context,
            now_secs(),
        )
        .await
        {
            UpdateState::Processed
        } else {
            UpdateState::Failed
        };
    }

    if let Some(intent) = parts.text.and_then(intent::parse) {
        return run_capture(
            database,
            item.bot_id,
            parts.chat_id,
            parts.sender_id,
            platform_api::CaptureSource::Url(intent.url),
            context,
            None,
        )
        .await;
    }
    handle_attachment(database, item.bot_id, &parts, context).await
}

/// Submit one capture intent and map the outcome to the update's terminal state.
async fn run_capture(
    database: &Database,
    bot_id: i64,
    chat_id: i64,
    telegram_user_id: i64,
    source: platform_api::CaptureSource,
    context: &CaptureContext,
    metadata: Option<telegram_persistence::intents::IntentMetadata>,
) -> UpdateState {
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
            UpdateState::Processed
        }
        Err(class) => {
            metrics::counter!(
                telegram_telemetry::metrics::TELEGRAM_CAPTURE_SUBMISSIONS_TOTAL,
                "class" => class.as_str(),
            )
            .increment(1);
            tracing::warn!(class = class.as_str(), "the capture could not be submitted");
            UpdateState::Failed
        }
    }
}

/// Handle the attachment alternative after URL parsing: supported files enter the bounded blob
/// path; unsupported kinds and declared oversize inputs receive one durable, truthful response.
async fn handle_attachment(
    database: &Database,
    bot_id: i64,
    parts: &MessageParts<'_>,
    context: &CaptureContext,
) -> UpdateState {
    let Some(choice) = select_attachment(parts.attachment.as_ref(), context.max_attachment_bytes)
    else {
        return UpdateState::Unsupported;
    };
    let choice = match choice {
        Ok(choice) => choice,
        Err(reply) => return enqueue_reply(database, bot_id, parts.chat_id, reply).await,
    };

    let file = match context.bot_api.get_file(&choice.file_id).await {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(error = %error, class = "attachment_file_resolution_failed", "Bot API file resolution failed");
            return UpdateState::Failed;
        }
    };
    let mut download = match context.bot_api.download_file(&file.path).await {
        Ok(download) => download,
        Err(error) => {
            tracing::warn!(error = %error, class = "attachment_download_failed", "Bot API attachment download failed");
            return UpdateState::Failed;
        }
    };
    let Ok(media_type) = MediaType::parse(&choice.media_type) else {
        return UpdateState::Failed;
    };
    let blob = match context
        .blobs
        .store(
            &media_type,
            &mut download,
            Some(context.max_attachment_bytes),
        )
        .await
    {
        Ok(blob) => blob,
        Err(error) => {
            tracing::warn!(error = %error, class = "attachment_store_failed", "attachment blob storage failed");
            return UpdateState::Failed;
        }
    };
    let metadata = telegram_persistence::intents::IntentMetadata {
        forward: parts.forward.clone(),
        blob: Some(telegram_persistence::intents::BlobCapture {
            owner_service: blob.owner_service.as_str().to_owned(),
            algorithm: "sha256".to_owned(),
            digest_hex: blob.digest.hex.as_str().to_owned(),
            media_type: blob.media_type.as_str().to_owned(),
            length_bytes: blob.length_bytes,
        }),
    };
    let source = platform_api::CaptureSource::Blob {
        owner_service: blob.owner_service.as_str().to_owned(),
        digest_hex: blob.digest.hex.as_str().to_owned(),
        media_type: blob.media_type.as_str().to_owned(),
        length_bytes: blob.length_bytes,
    };
    run_capture(
        database,
        bot_id,
        parts.chat_id,
        parts.sender_id,
        source,
        context,
        Some(metadata),
    )
    .await
}

/// Pick the largest photo that can enter the configured budget; a document has one declared size.
fn select_attachment(
    attachment: Option<&MessageAttachment>,
    limit: u64,
) -> Option<Result<AttachmentChoice, ReplyKind>> {
    match attachment? {
        MessageAttachment::Document {
            file_id,
            declared_bytes,
            media_type,
        } if *declared_bytes <= limit => Some(Ok(AttachmentChoice {
            file_id: file_id.clone(),
            media_type: media_type.clone(),
        })),
        MessageAttachment::Document { .. } => Some(Err(ReplyKind::TooLarge { limit })),
        MessageAttachment::Photos(photos) => photos
            .iter()
            .filter(|photo| photo.declared_bytes <= limit)
            .max_by_key(|photo| photo.declared_bytes)
            .map(|photo| {
                Ok(AttachmentChoice {
                    file_id: photo.file_id.clone(),
                    media_type: "image/jpeg".to_owned(),
                })
            })
            .or(Some(Err(ReplyKind::TooLarge { limit }))),
        MessageAttachment::Unsupported => Some(Err(ReplyKind::Unsupported)),
    }
}

/// Persist a single static reply. It deliberately does not touch Platform or Bot API directly.
async fn enqueue_reply(
    database: &Database,
    bot_id: i64,
    chat_id: i64,
    reply: ReplyKind,
) -> UpdateState {
    let payload = telegram_persistence::outbound_jobs::MessagePayload {
        text: reply.body(),
        parse_mode: Some("HTML".to_owned()),
        reply_markup: None,
    };
    let Ok(content_hash) = payload.canonical() else {
        return UpdateState::Failed;
    };
    let result = database
        .enqueue_outbound_job(
            &telegram_persistence::outbound_jobs::NewOutboundJob {
                bot_id,
                chat_id,
                kind: telegram_persistence::outbound_jobs::OutboundJobKind::SendMessage,
                payload,
                content_hash,
                operation_id: None,
                revision: None,
                correlation_id: None,
                next_attempt_at: None,
            },
            now_secs(),
        )
        .await;
    if result.is_ok() {
        UpdateState::Processed
    } else {
        UpdateState::Failed
    }
}

/// Static response classes: no untrusted MIME, filename, or Telegram content reaches the reply.
enum ReplyKind {
    Unsupported,
    TooLarge { limit: u64 },
}

impl ReplyKind {
    fn body(&self) -> String {
        match self {
            Self::Unsupported => {
                "<b>Unsupported attachment</b>\nSend a PDF document or photo; video, voice, and audio are not supported yet.".to_owned()
            }
            Self::TooLarge { limit } => format!(
                "<b>Attachment too large</b>\nSend a PDF document or photo up to {limit} bytes."
            ),
        }
    }
}

/// A worker timestamp only schedules a ready outbound job; no Telegram request occurs here.
fn now_secs() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// The selected file ready for a Bot API `getFile` request.
struct AttachmentChoice {
    file_id: String,
    media_type: String,
}

/// The pieces of a message update the domain action reads: its text, sender, chat, caption, and
/// minimized forward origin when the message is one.
struct MessageParts<'a> {
    text: Option<&'a str>,
    caption: Option<&'a str>,
    sender_id: i64,
    chat_id: i64,
    forward: Option<telegram_persistence::intents::CaptureOrigin>,
    attachment: Option<MessageAttachment>,
}

/// The minimized attachment metadata Telegram supplies before a download starts.
enum MessageAttachment {
    Document {
        file_id: String,
        declared_bytes: u64,
        media_type: String,
    },
    Photos(Vec<PhotoCandidate>),
    Unsupported,
}

/// One photo rendition Telegram made available on the message.
struct PhotoCandidate {
    file_id: String,
    declared_bytes: u64,
}

fn message_parts(kind: &bot_api::UpdateKind) -> Option<MessageParts<'_>> {
    let (bot_api::UpdateKind::Message(message) | bot_api::UpdateKind::EditedMessage(message)) =
        kind
    else {
        return None;
    };
    let sender = message.from.as_ref()?;
    Some(MessageParts {
        text: message.text(),
        caption: message.caption(),
        sender_id: i64::try_from(sender.id.0).ok()?,
        chat_id: message.chat.id.0,
        forward: message.forward_origin().and_then(minimize_origin),
        attachment: message_attachment(message),
    })
}

/// Identify supported attachment metadata without downloading it. All unsupported media shares
/// one static reply; only its existence is relevant, not untrusted provider labels.
fn message_attachment(message: &bot_api::Message) -> Option<MessageAttachment> {
    if let Some(document) = message.document() {
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "Mime is a Teloxide-owned type; keeping its crate out of this boundary avoids a direct transitive dependency"
        )]
        let Some(media_type) = document.mime_type.as_ref().map(|mime| mime.essence_str()) else {
            return Some(MessageAttachment::Unsupported);
        };
        return Some(if media_type == "application/pdf" {
            MessageAttachment::Document {
                file_id: document.file.id.0.clone(),
                declared_bytes: u64::from(document.file.size),
                media_type: media_type.to_owned(),
            }
        } else {
            MessageAttachment::Unsupported
        });
    }
    if let Some(photos) = message.photo() {
        return Some(MessageAttachment::Photos(
            photos
                .iter()
                .map(|photo| PhotoCandidate {
                    file_id: photo.file.id.0.clone(),
                    declared_bytes: u64::from(photo.file.size),
                })
                .collect(),
        ));
    }
    if message.voice().is_some()
        || message.video().is_some()
        || message.audio().is_some()
        || message.animation().is_some()
        || message.video_note().is_some()
        || message.sticker().is_some()
    {
        return Some(MessageAttachment::Unsupported);
    }
    None
}

/// Provenance for a URL capture contains only the forward facts and never a blob marker.
fn url_metadata(
    forward: telegram_persistence::intents::CaptureOrigin,
) -> telegram_persistence::intents::IntentMetadata {
    telegram_persistence::intents::IntentMetadata {
        forward: Some(forward),
        blob: None,
    }
}

/// Minimize a forwarded post's origin to identifiers, kind, and original date. Sender fields are
/// untrusted input; nothing beyond these facts is stored or submitted.
fn minimize_origin(
    origin: &bot_api::MessageOrigin,
) -> Option<telegram_persistence::intents::CaptureOrigin> {
    use bot_api::MessageOrigin as Origin;
    use telegram_persistence::intents::CaptureOrigin;

    let sent_at_secs = origin.date().timestamp();
    match origin {
        Origin::User { sender_user, .. } => Some(CaptureOrigin::User {
            user_id: i64::try_from(sender_user.id.0).ok()?,
            sent_at_secs,
        }),
        Origin::HiddenUser {
            sender_user_name, ..
        } => Some(CaptureOrigin::HiddenUser {
            sender_name: sender_user_name.clone(),
            sent_at_secs,
        }),
        Origin::Chat { sender_chat, .. } => Some(CaptureOrigin::Chat {
            chat_id: sender_chat.id.0,
            sent_at_secs,
        }),
        Origin::Channel {
            chat, message_id, ..
        } => Some(CaptureOrigin::Channel {
            chat_id: chat.id.0,
            message_id: i64::from(message_id.0),
            sent_at_secs,
        }),
    }
}

/// The first external http(s) link in untrusted free text, if any. The link ends at whitespace or
/// a common enclosing punctuation character; trailing sentence punctuation is trimmed.
fn first_https_link(text: &str) -> Option<String> {
    for (start, _) in text.match_indices("http") {
        let Some(candidate) = text.get(start..) else {
            continue;
        };
        let scheme_len = if candidate.starts_with("https://") {
            "https://".len()
        } else if candidate.starts_with("http://") {
            "http://".len()
        } else {
            continue;
        };
        let end = candidate
            .find([' ', '\t', '\n', '\r', '"', '\'', '>', ')'])
            .unwrap_or(candidate.len());
        let Some(raw) = candidate.get(..end) else {
            continue;
        };
        let trimmed = raw.trim_end_matches(['.', ',', ';', '!', '?']);
        if trimmed.len() > scheme_len {
            return Some(trimmed.to_owned());
        }
    }
    None
}
