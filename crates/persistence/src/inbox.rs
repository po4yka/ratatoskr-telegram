//! Inbox deduplication: the at-least-once event ledger every consumed envelope goes through.
//!
//! The contracts envelope names each event occurrence with a globally unique `event_id`, and an
//! at-least-once transport WILL redeliver. Like update admission, the insert IS the decision —
//! `insert ... on conflict do nothing returning true` yields a row exactly when this call is the
//! first, so deduplication is atomic with the evidence and needs no read-before-write.

use sqlx::types::Uuid;

use crate::{Database, PersistenceError, RecordOutcome};

impl Database {
    /// Record one consumed envelope id; decide acceptance by insertion.
    ///
    /// The first arrival inserts and reports [`RecordOutcome::Inserted`]; every redelivery of the
    /// same `event_id` reports [`RecordOutcome::Duplicate`] without touching anything else in the
    /// schema. The caller owns what "already handled" means beyond this ledger.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails.
    pub async fn record_event(&self, event_id: Uuid) -> Result<RecordOutcome, PersistenceError> {
        let inserted: Option<bool> = sqlx::query_scalar(
            "insert into telegram.inbox (event_id)
             values ($1)
             on conflict (event_id) do nothing
             returning true",
        )
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        Ok(if inserted.is_some() {
            RecordOutcome::Inserted
        } else {
            RecordOutcome::Duplicate
        })
    }
}
