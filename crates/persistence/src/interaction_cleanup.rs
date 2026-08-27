//! Bounded expiry and retention cleanup for Telegram interaction authority.

use crate::{Database, PersistenceError};

fn checked_count(value: i64) -> Result<u64, PersistenceError> {
    u64::try_from(value)
        .map_err(|error| PersistenceError::Query(sqlx::Error::Decode(Box::new(error))))
}

/// Rows changed by one bounded cleanup transaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractionCleanupCounts {
    /// Active dialogues transitioned to `expired`.
    pub dialogues_expired: u64,
    /// Expired, consumed, or stale interaction tokens removed.
    pub tokens_deleted: u64,
    /// Retention-expired terminal dialogues removed after their tokens.
    pub dialogues_deleted: u64,
}

impl Database {
    /// Apply one bounded interaction cleanup transaction.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if storage refuses the cleanup.
    pub async fn cleanup_interactions(
        &self,
        now: i64,
        terminal_before: i64,
        batch_size: u32,
    ) -> Result<InteractionCleanupCounts, PersistenceError> {
        let limit = i64::from(batch_size);
        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;

        let dialogues_expired: i64 = sqlx::query_scalar(
            "with candidates as (
                 select id from telegram.dialog_states
                 where lifecycle = 'active' and expires_at <= to_timestamp($1)
                 order by expires_at, id
                 for update skip locked
                 limit $2
             ), expired as (
                 update telegram.dialog_states as dialogue
                 set lifecycle = 'expired', terminal_at = to_timestamp($1),
                     updated_at = to_timestamp($1), version = dialogue.version + 1
                 from candidates
                 where dialogue.id = candidates.id
                 returning 1
             )
             select count(*)::bigint from expired",
        )
        .bind(now)
        .bind(limit)
        .fetch_one(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;

        let tokens_deleted: i64 = sqlx::query_scalar(
            "with candidates as (
                 select token.token
                 from telegram.interaction_tokens as token
                 left join telegram.dialog_states as dialogue on dialogue.id = token.dialogue_id
                 where (
                       token.consumed_at is not null
                    or token.expires_at <= to_timestamp($1)
                    or (token.dialogue_id is not null and (
                        dialogue.id is null
                        or dialogue.lifecycle <> 'active'
                        or dialogue.version <> token.expected_dialogue_version
                    )))
                   and not (
                       token.surface = 'deep_link'
                       and exists (
                           select 1 from telegram.message_bindings as binding
                           where binding.operation_id = token.operation_id
                             and not binding.terminal
                       )
                   )
                 order by token.expires_at, token.token
                 for update of token skip locked
                 limit $2
             ), removed as (
                 delete from telegram.interaction_tokens as token
                 using candidates
                 where token.token = candidates.token
                 returning 1
             )
             select count(*)::bigint from removed",
        )
        .bind(now)
        .bind(limit)
        .fetch_one(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;

        let dialogues_deleted: i64 = sqlx::query_scalar(
            "with candidates as (
                 select dialogue.id
                 from telegram.dialog_states as dialogue
                 where dialogue.lifecycle <> 'active'
                   and dialogue.terminal_at < to_timestamp($1)
                   and not exists (
                       select 1 from telegram.interaction_tokens as token
                       where token.dialogue_id = dialogue.id
                   )
                 order by dialogue.terminal_at, dialogue.id
                 for update of dialogue skip locked
                 limit $2
             ), removed as (
                 delete from telegram.dialog_states as dialogue
                 using candidates
                 where dialogue.id = candidates.id
                 returning 1
             )
             select count(*)::bigint from removed",
        )
        .bind(terminal_before)
        .bind(limit)
        .fetch_one(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;

        let counts = InteractionCleanupCounts {
            dialogues_expired: checked_count(dialogues_expired)?,
            tokens_deleted: checked_count(tokens_deleted)?,
            dialogues_deleted: checked_count(dialogues_deleted)?,
        };
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(counts)
    }
}
