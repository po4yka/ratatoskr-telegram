//! Deep-link intent records: opaque, expiring, owner-bound rows behind Mini App links.
//!
//! The identifier IS the opaque token - the application mints it (`UUIDv7`, no database default)
//! and it appears alone in the deep-link parameter. Lookups match only unexpired rows and only
//! the owning Telegram user, so a forwarded link resolves to nothing for anyone else.

use sqlx::types::Uuid;

use crate::{Database, PersistenceError};

/// The one intent kind this flow writes. Every kind is a product decision, so the vocabulary is
/// closed and grows only when a new flow owns its writer.
pub const OPERATION_STATUS_KIND: &str = "operation_status";

/// Where a forwarded message came from, minimized to identifiers, kind, and original date.
///
/// A forwarded post's sender fields are untrusted input; this record keeps only what provenance
/// needs - never display text beyond a hidden sender's name, never message content.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureOrigin {
    /// Forwarded from a user account.
    User {
        /// The forwarding user's Telegram id.
        user_id: i64,
        /// When the original was sent, whole seconds since the Unix epoch.
        sent_at_secs: i64,
    },
    /// Forwarded with the sender hidden; only a display name is known.
    HiddenUser {
        /// The name Telegram shows for the hidden sender.
        sender_name: String,
        /// When the original was sent, whole seconds since the Unix epoch.
        sent_at_secs: i64,
    },
    /// Forwarded from a chat.
    Chat {
        /// The originating chat's Telegram id.
        chat_id: i64,
        /// When the original was sent, whole seconds since the Unix epoch.
        sent_at_secs: i64,
    },
    /// Forwarded from a channel.
    Channel {
        /// The channel's Telegram id.
        chat_id: i64,
        /// The original message's id inside that channel.
        message_id: i64,
        /// When the original was sent, whole seconds since the Unix epoch.
        sent_at_secs: i64,
    },
}

/// The stored-bytes facts an attachment capture presents instead of an address. Field-for-field
/// the fleet `BlobRef` wire shape, kept local so persistence does not depend on the contracts crate;
/// conversion happens at the submission boundary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobCapture {
    /// The owner service whose store holds the bytes (`ratatoskr-telegram` here).
    pub owner_service: String,
    /// The digest algorithm name (`sha256`).
    pub algorithm: String,
    /// The lowercase hex digest of the exact stored bytes.
    pub digest_hex: String,
    /// The parameterless media type of the stored artifact.
    pub media_type: String,
    /// The stored byte length.
    pub length_bytes: u64,
}

/// Bounded capture-provenance facts one intent row may carry: where a forward came from, or what
/// stored blob an attachment capture presents. A closed shape - unknown members are refused -
/// because this column sits beside user data and must never become a free-form bag.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentMetadata {
    /// Forward origin facts, when the input arrived as a forward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<CaptureOrigin>,
    /// Stored-blob facts, when the capture presents an attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<BlobCapture>,
}

/// A new intent record. The caller mints `id` because the identifier is the token clients see.
#[derive(Debug, Clone)]
pub struct NewIntent {
    /// The app-minted opaque token.
    pub id: Uuid,
    /// The bot the deep link addresses.
    pub bot_id: i64,
    /// The owning Telegram user; lookups are owner-scoped.
    pub telegram_user_id: i64,
    /// The chat the intent was created from.
    pub chat_id: i64,
    /// The Platform operation the intent presents.
    pub operation_id: Uuid,
    /// The submitted address the intent presents; absent for attachment captures.
    pub source_url: Option<String>,
    /// Bounded provenance facts, when the capture carries them.
    pub metadata: Option<IntentMetadata>,
    /// When the intent stops resolving, whole seconds since the Unix epoch.
    pub expires_at_secs: i64,
}

/// One stored intent, as a resolver sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentRecord {
    /// The opaque token (`id`).
    pub id: Uuid,
    /// The bot the deep link addresses.
    pub bot_id: i64,
    /// The chat the intent was created from.
    pub chat_id: i64,
    /// The Platform operation the intent presents.
    pub operation_id: Uuid,
    /// The submitted address the intent presents; absent for attachment captures.
    pub source_url: Option<String>,
    /// Bounded provenance facts, when the row carries them.
    pub metadata: Option<IntentMetadata>,
}

type IntentRow = (
    Uuid,
    i64,
    i64,
    Uuid,
    Option<String>,
    Option<serde_json::Value>,
);

impl IntentRecord {
    fn from_parts(
        id: Uuid,
        bot_id: i64,
        chat_id: i64,
        operation_id: Uuid,
        source_url: Option<String>,
        metadata: Option<serde_json::Value>,
    ) -> Self {
        Self {
            id,
            bot_id,
            chat_id,
            operation_id,
            source_url,
            // Unreadable provenance must never break resolution or rendering; it degrades to
            // absent with its class logged.
            metadata: metadata.and_then(parse_metadata),
        }
    }
}

/// Decode one stored metadata value into the closed shape.
fn parse_metadata(raw: serde_json::Value) -> Option<IntentMetadata> {
    match serde_json::from_value(raw) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            tracing::warn!(
                class = "intent_metadata_invalid",
                error = %error,
                "an intent row carried unreadable provenance; treating it as absent"
            );
            None
        }
    }
}

impl Database {
    /// Store one intent record.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails - including the table's own refusal of
    /// a row that presents neither an address nor blob facts.
    pub async fn insert_intent(
        &self,
        intent: &NewIntent,
        now: i64,
    ) -> Result<(), PersistenceError> {
        let metadata = intent
            .metadata
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| {
                PersistenceError::Query(sqlx::Error::ColumnDecode {
                    index: "metadata".to_owned(),
                    source: Box::new(error),
                })
            })?;
        sqlx::query(
            "insert into telegram.interaction_intents
                 (id, bot_id, telegram_user_id, chat_id, kind, operation_id, source_url,
                  metadata, created_at, expires_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, to_timestamp($9), to_timestamp($10))",
        )
        .bind(intent.id)
        .bind(intent.bot_id)
        .bind(intent.telegram_user_id)
        .bind(intent.chat_id)
        .bind(OPERATION_STATUS_KIND)
        .bind(intent.operation_id)
        .bind(intent.source_url.as_deref())
        .bind(metadata)
        .bind(now)
        .bind(intent.expires_at_secs)
        .execute(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;
        Ok(())
    }

    /// Resolve an intent for its owner while it is live; expired rows and other users' rows are
    /// indistinguishable absences.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails.
    pub async fn find_live_intent(
        &self,
        id: Uuid,
        telegram_user_id: i64,
        now: i64,
    ) -> Result<Option<IntentRecord>, PersistenceError> {
        let row: Option<IntentRow> = sqlx::query_as(
            "select id, bot_id, chat_id, operation_id, source_url, metadata
             from telegram.interaction_intents
             where id = $1
               and telegram_user_id = $2
               and kind = $3
               and expires_at > to_timestamp($4)",
        )
        .bind(id)
        .bind(telegram_user_id)
        .bind(OPERATION_STATUS_KIND)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;
        Ok(row.map(
            |(id, bot_id, chat_id, operation_id, source_url, metadata)| {
                IntentRecord::from_parts(id, bot_id, chat_id, operation_id, source_url, metadata)
            },
        ))
    }

    /// The live intent recorded for one operation, whichever owner it was created for. Render
    /// composition reads this; authorization stays with owner-scoped resolution.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails.
    pub async fn find_live_intent_by_operation(
        &self,
        operation_id: Uuid,
        now: i64,
    ) -> Result<Option<IntentRecord>, PersistenceError> {
        let row: Option<IntentRow> = sqlx::query_as(
            "select id, bot_id, chat_id, operation_id, source_url, metadata
             from telegram.interaction_intents
             where operation_id = $1
               and kind = $2
               and expires_at > to_timestamp($3)",
        )
        .bind(operation_id)
        .bind(OPERATION_STATUS_KIND)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;
        Ok(row.map(
            |(id, bot_id, chat_id, operation_id, source_url, metadata)| {
                IntentRecord::from_parts(id, bot_id, chat_id, operation_id, source_url, metadata)
            },
        ))
    }

    /// The Telegram user an operation's intent was created for - the identity a follower
    /// authenticates as to stream that operation. Unfiltered by expiry: following outlives the
    /// link's presentation life.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails.
    pub async fn find_intent_owner_by_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<i64>, PersistenceError> {
        let row: Option<(i64,)> = sqlx::query_as(
            "select telegram_user_id from telegram.interaction_intents
             where operation_id = $1 and kind = $2",
        )
        .bind(operation_id)
        .bind(OPERATION_STATUS_KIND)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;
        Ok(row.map(|(owner,)| owner))
    }
}
