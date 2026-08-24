//! Update deduplication state: the `(bot_id, update_id)` ledger every admission decision goes
//! through, and the processing-state transitions the worker owns.
//!
//! The insert IS the deduplication decision — exact match over the composite key, never a
//! high-water mark, because Telegram can redeliver old ids after reconnects and an unseen id below
//! the highest seen id is still new input.

use crate::{Database, PersistenceError};

/// An admitted update as the intake records it, including its authenticated processable payload.
#[derive(Debug, Clone)]
pub struct AdmittedUpdate {
    /// The bot's user id, learned from `getMe` at startup.
    pub bot_id: i64,
    /// `update_id` as delivered.
    pub update_id: i64,
    /// The classification label (`message`, `callback_query`, ..., `unsupported`).
    pub kind: String,
    /// The parsed Bot API update serialized as JSON.
    pub payload: String,
}

/// One durable update claimed for processing.
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    /// The receiving bot.
    pub bot_id: i64,
    /// Telegram's update identity.
    pub update_id: i64,
    /// The parsed Bot API update serialized as JSON.
    pub payload: String,
}

/// What one [`Database::record_update`] decided. `Duplicate` is not an error: a redelivery is
/// answered 200 and dropped, which is exactly what the caller wants to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    /// The pair was unseen; the row exists now and processing may proceed.
    Inserted,
    /// The pair was already admitted; nothing changed and the delivery must be dropped.
    Duplicate,
}

/// A terminal (or in-flight) processing state the worker moves a row through.
///
/// `Accepted` is the insert default and deliberately absent here: only the admission path writes
/// it, and only via the column default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    /// Picked up by the worker; between the two writes of one settlement.
    Processing,
    /// A kind this build acts on, acted on.
    Processed,
    /// A well-formed update of a kind this build does not act on yet.
    Unsupported,
    /// Processing failed after acceptance; the row records it rather than hiding it.
    Failed,
    /// The access policy refused the sender or chat before any processing ran. The webhook
    /// decides this; this layer only records the outcome.
    Denied,
}

impl UpdateState {
    /// The closed-vocabulary string stored in the column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Processing => "processing",
            Self::Processed => "processed",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::Denied => "denied",
        }
    }
}

impl Database {
    /// Record an admitted update; decide acceptance by insertion.
    ///
    /// One statement does both jobs: `insert ... on conflict do nothing returning true` yields a
    /// row exactly when this call inserted the row, so the decision is atomic with the write — no
    /// check-then-insert race between two deliveries of the same update.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails.
    pub async fn record_update(
        &self,
        update: &AdmittedUpdate,
    ) -> Result<RecordOutcome, PersistenceError> {
        let inserted: Option<bool> = sqlx::query_scalar(
            "insert into telegram.updates (bot_id, update_id, kind, payload)
             values ($1, $2, $3, $4::jsonb)
             on conflict (bot_id, update_id) do nothing
             returning true",
        )
        .bind(update.bot_id)
        .bind(update.update_id)
        .bind(&update.kind)
        .bind(&update.payload)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        Ok(if inserted.is_some() {
            RecordOutcome::Inserted
        } else {
            RecordOutcome::Duplicate
        })
    }

    /// Claim the oldest processable update, including one interrupted during processing.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the claim fails.
    pub async fn claim_update(&self) -> Result<Option<PendingUpdate>, PersistenceError> {
        let row: Option<(i64, i64, String)> = sqlx::query_as(
            "with pending as (
                 select bot_id, update_id
                 from telegram.updates
                 where state in ('accepted', 'processing') and payload is not null
                 order by received_at, bot_id, update_id
                 for update skip locked
                 limit 1
             )
             update telegram.updates as updates
             set state = 'processing'
             from pending
             where updates.bot_id = pending.bot_id and updates.update_id = pending.update_id
             returning updates.bot_id, updates.update_id, updates.payload::text",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        Ok(row.map(|(bot_id, update_id, payload)| PendingUpdate {
            bot_id,
            update_id,
            payload,
        }))
    }

    /// Move an admitted update's state forward, stamping the settle time on terminal states.
    ///
    /// Two calls make one settlement for the worker: first to [`UpdateState::Processing`], then to
    /// the terminal state. A settlement naming a pair that was never admitted FAILS rather than
    /// writing — a state transition for an update that does not exist is a bug, and silently
    /// succeeding would hide it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::UnknownUpdate`] when no row carries the pair;
    /// [`PersistenceError::Query`] if the statement fails otherwise.
    pub async fn settle_update(
        &self,
        bot_id: i64,
        update_id: i64,
        state: UpdateState,
    ) -> Result<(), PersistenceError> {
        let result = sqlx::query(
            "update telegram.updates
             set state = $3,
                 settled_at = case when $3 in ('processed', 'unsupported', 'failed', 'denied')
                                   then now() else settled_at end,
                 payload = case when $3 in ('processed', 'unsupported', 'failed', 'denied')
                                then null else payload end
             where bot_id = $1 and update_id = $2",
        )
        .bind(bot_id)
        .bind(update_id)
        .bind(state.as_str())
        .execute(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        if result.rows_affected() == 0 {
            return Err(PersistenceError::UnknownUpdate);
        }
        Ok(())
    }
}
