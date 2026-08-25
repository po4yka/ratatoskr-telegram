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
    /// The submitted address the intent presents.
    pub source_url: String,
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
    /// The submitted address the intent presents.
    pub source_url: String,
}

impl Database {
    /// Store one intent record.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails.
    pub async fn insert_intent(
        &self,
        intent: &NewIntent,
        now: i64,
    ) -> Result<(), PersistenceError> {
        sqlx::query(
            "insert into telegram.interaction_intents
                 (id, bot_id, telegram_user_id, chat_id, kind, operation_id, source_url,
                  created_at, expires_at)
             values ($1, $2, $3, $4, $5, $6, $7, to_timestamp($8), to_timestamp($9))",
        )
        .bind(intent.id)
        .bind(intent.bot_id)
        .bind(intent.telegram_user_id)
        .bind(intent.chat_id)
        .bind(OPERATION_STATUS_KIND)
        .bind(intent.operation_id)
        .bind(&intent.source_url)
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
        let row: Option<(Uuid, i64, i64, Uuid, String)> = sqlx::query_as(
            "select id, bot_id, chat_id, operation_id, source_url
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
            |(id, bot_id, chat_id, operation_id, source_url)| IntentRecord {
                id,
                bot_id,
                chat_id,
                operation_id,
                source_url,
            },
        ))
    }
}
