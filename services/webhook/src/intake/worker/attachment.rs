//! The attachment half of the capture arm: which attachments enter the blob path, and the static
//! replies for everything that does not (XR-021 CONTRACTS S11).
//!
//! Only a document whose declared media type is `application/pdf` is stored and submitted. Photos
//! are refusal-only until an OCR capture path exists, and every other kind gets the same static
//! unsupported reply. No untrusted MIME type, filename, or Telegram content reaches a reply.

use ratatoskr_identifiers::MediaType;
use telegram_persistence::{Database, UpdateState};

use super::{MessageParts, ProcessingOutcome, capture_run, enqueue_text};
use crate::intake::worker::CaptureContext;

/// The only attachment media type that enters the blob path.
const PDF_MEDIA_TYPE: &str = "application/pdf";

/// The minimized attachment metadata Telegram supplies before a download starts.
pub(super) enum MessageAttachment {
    /// A document with the media type Telegram declared for it.
    Document {
        file_id: String,
        declared_bytes: u64,
        media_type: String,
    },
    /// Anything else: photos, video, voice, audio, stickers, or a document without a media type.
    Unsupported,
}

/// The selected file ready for a Bot API `getFile` request.
struct AttachmentChoice {
    file_id: String,
}

/// Static response classes: no untrusted MIME, filename, or Telegram content reaches the reply.
pub(super) enum ReplyKind {
    /// The attachment kind is not accepted.
    Unsupported,
    /// The declared size exceeds the configured attachment budget.
    TooLarge { limit: u64 },
    /// Platform stayed unavailable through the bounded retries; nothing was saved.
    CaptureUnavailable,
    /// Platform permanently refused the capture; nothing was saved.
    CaptureRefused,
    /// The attachment could not be fetched, read, or stored.
    AttachmentUnreadable,
}

impl ReplyKind {
    fn body(&self) -> String {
        match self {
            Self::Unsupported => "<b>Unsupported attachment</b>\nSend a PDF document with a text layer; images, video, voice, and audio are not supported yet.".to_owned(),
            Self::TooLarge { limit } => format!(
                "<b>Attachment too large</b>\nSend a PDF document up to {limit} bytes."
            ),
            Self::CaptureUnavailable => "<b>Ratatoskr is temporarily unavailable</b>\nYour item was not saved. Please send it again in a few minutes.".to_owned(),
            Self::CaptureRefused => "<b>Could not save this item</b>\nRatatoskr did not accept it. If this keeps happening, contact the operator.".to_owned(),
            Self::AttachmentUnreadable => "<b>Could not read the attachment</b>\nPlease send it again.".to_owned(),
        }
    }
}

/// Persist one static reply and settle the update `Failed`, whether or not the reply was stored.
///
/// A failure the sender caused or must retry is never silent; a failed enqueue cannot soften the
/// terminal state, so the update stays `Failed` either way.
pub(super) async fn reply_and_fail(
    database: &Database,
    bot_id: i64,
    chat_id: i64,
    reply: ReplyKind,
) -> ProcessingOutcome {
    if enqueue_reply(database, bot_id, chat_id, reply).await != UpdateState::Processed {
        tracing::warn!(
            class = "failure_reply_not_enqueued",
            "the static failure reply could not be stored",
        );
    }
    UpdateState::Failed.into()
}

/// Persist a single static reply. It deliberately does not touch Platform or Bot API directly.
async fn enqueue_reply(
    database: &Database,
    bot_id: i64,
    chat_id: i64,
    reply: ReplyKind,
) -> UpdateState {
    enqueue_text(database, bot_id, chat_id, &reply.body()).await
}

/// Handle the attachment alternative after URL parsing: a PDF enters the bounded blob path;
/// everything else, and a declared oversize input, receives one durable, truthful reply.
pub(super) async fn handle_attachment(
    database: &Database,
    bot_id: i64,
    parts: &MessageParts<'_>,
    context: &CaptureContext,
) -> ProcessingOutcome {
    let Some(choice) = select_attachment(parts.attachment.as_ref(), context.max_attachment_bytes)
    else {
        return UpdateState::Unsupported.into();
    };
    let choice = match choice {
        Ok(choice) => choice,
        Err(reply) => {
            return enqueue_reply(database, bot_id, parts.chat_id, reply)
                .await
                .into();
        }
    };

    let file = match context.bot_api.get_file(&choice.file_id).await {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(error = %error, class = "attachment_file_resolution_failed", "Bot API file resolution failed");
            return reply_and_fail(
                database,
                bot_id,
                parts.chat_id,
                ReplyKind::AttachmentUnreadable,
            )
            .await;
        }
    };
    let mut download = match context.bot_api.download_file(&file.path).await {
        Ok(download) => download,
        Err(error) => {
            tracing::warn!(error = %error, class = "attachment_download_failed", "Bot API attachment download failed");
            return reply_and_fail(
                database,
                bot_id,
                parts.chat_id,
                ReplyKind::AttachmentUnreadable,
            )
            .await;
        }
    };
    let Ok(media_type) = MediaType::parse(PDF_MEDIA_TYPE) else {
        tracing::warn!(
            class = "attachment_media_type_invalid",
            "the PDF media type did not parse"
        );
        return reply_and_fail(
            database,
            bot_id,
            parts.chat_id,
            ReplyKind::AttachmentUnreadable,
        )
        .await;
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
            return reply_and_fail(
                database,
                bot_id,
                parts.chat_id,
                ReplyKind::AttachmentUnreadable,
            )
            .await;
        }
    };
    let metadata = telegram_persistence::interaction_tokens::IntentMetadata {
        forward: parts.forward.clone(),
        blob: Some(telegram_persistence::interaction_tokens::BlobCapture {
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
    capture_run::run_capture(
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

/// Accept a PDF document within the byte budget; refuse every other attachment with a static reply.
fn select_attachment(
    attachment: Option<&MessageAttachment>,
    limit: u64,
) -> Option<Result<AttachmentChoice, ReplyKind>> {
    match attachment? {
        MessageAttachment::Document {
            file_id,
            declared_bytes,
            media_type,
        } if media_type == PDF_MEDIA_TYPE => Some(if *declared_bytes <= limit {
            Ok(AttachmentChoice {
                file_id: file_id.clone(),
            })
        } else {
            Err(ReplyKind::TooLarge { limit })
        }),
        MessageAttachment::Document { .. } | MessageAttachment::Unsupported => {
            Some(Err(ReplyKind::Unsupported))
        }
    }
}

/// Identify attachment metadata without downloading it. The kind decides acceptance; untrusted
/// provider labels never reach a reply.
pub(super) fn message_attachment(message: &bot_api::Message) -> Option<MessageAttachment> {
    if let Some(document) = message.document() {
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "Mime is a Teloxide-owned type; keeping its crate out of this boundary avoids a direct transitive dependency"
        )]
        let Some(media_type) = document.mime_type.as_ref().map(|mime| mime.essence_str()) else {
            return Some(MessageAttachment::Unsupported);
        };
        return Some(MessageAttachment::Document {
            file_id: document.file.id.0.clone(),
            declared_bytes: u64::from(document.file.size),
            media_type: media_type.to_owned(),
        });
    }
    if message.photo().is_some()
        || message.voice().is_some()
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
