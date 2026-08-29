//! Atomic persistence of a provider-known outbound acknowledgement.

use sqlx::Row as _;

use crate::outbound_jobs::{AcknowledgedMethod, QueuedOutboundJob};
use crate::{Database, PersistenceError};

impl Database {
    /// Mark which concrete Bot API method this claim will place on the wire.
    ///
    /// A missing-message edit may become a fresh send. Persisting that downgrade before the call
    /// makes later lease recovery quarantine it under the same rule as every other send.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::StaleOutboundAcknowledgement`] when the claim no longer owns the job,
    /// or [`PersistenceError::Query`] when storage fails.
    pub async fn prepare_outbound_method(
        &self,
        job: &QueuedOutboundJob,
        method: AcknowledgedMethod,
        now: i64,
    ) -> Result<(), PersistenceError> {
        let result = sqlx::query(
            "update telegram.outbound_jobs
             set kind = $3, updated_at = to_timestamp($4)
             where id = $1 and state = 'sending' and attempts = $2",
        )
        .bind(job.id)
        .bind(job.attempts)
        .bind(method.as_str())
        .bind(now)
        .execute(self.pool())
        .await
        .map_err(PersistenceError::Query)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(PersistenceError::StaleOutboundAcknowledgement)
        }
    }

    /// Atomically record every local effect of one successful Bot API write.
    ///
    /// Repeating this operation with the same message id is safe after an uncertain database
    /// commit. A different acknowledgement or an obsolete claim is rejected.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::UnknownOutboundJob`] for an absent job,
    /// [`PersistenceError::StaleOutboundAcknowledgement`] for conflicting durable state, or
    /// [`PersistenceError::Query`] when the transaction fails.
    pub async fn record_outbound_acknowledgement(
        &self,
        job: &QueuedOutboundJob,
        method: AcknowledgedMethod,
        message_id: i64,
        now: i64,
    ) -> Result<(), PersistenceError> {
        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;
        let row = sqlx::query(
            "select state, attempts, acknowledged_message_id
             from telegram.outbound_jobs where id = $1 for update",
        )
        .bind(job.id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?
        .ok_or(PersistenceError::UnknownOutboundJob)?;
        let state: &str = row.get("state");
        let stored_message_id: Option<i64> = row.get("acknowledged_message_id");
        if state == "sent" && stored_message_id == Some(message_id) {
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(());
        }
        if state != "sending" || row.get::<i32, _>("attempts") != job.attempts {
            return Err(PersistenceError::StaleOutboundAcknowledgement);
        }

        apply_callback_stamp(&mut transaction, job, message_id, now).await?;
        apply_binding_ack(&mut transaction, job, method, message_id, now).await?;
        apply_notification_ack(&mut transaction, job, now).await?;
        let settled = sqlx::query(
            "update telegram.outbound_jobs
             set state = 'sent', acknowledged_message_id = $3, lease_expires_at = null,
                 last_error_class = null, updated_at = to_timestamp($4)
             where id = $1 and state = 'sending' and attempts = $2",
        )
        .bind(job.id)
        .bind(job.attempts)
        .bind(message_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if settled.rows_affected() != 1 {
            return Err(PersistenceError::StaleOutboundAcknowledgement);
        }
        transaction.commit().await.map_err(PersistenceError::Query)
    }
}

async fn apply_callback_stamp(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job: &QueuedOutboundJob,
    message_id: i64,
    now: i64,
) -> Result<(), PersistenceError> {
    let Some(dialogue_id) = job
        .correlation_id
        .as_deref()
        .and_then(|value| value.strip_prefix("telegram-dialogue:"))
        .and_then(|value| value.parse::<sqlx::types::Uuid>().ok())
    else {
        return Ok(());
    };
    let version: Option<i64> = sqlx::query_scalar(
        "update telegram.dialog_states
         set expected_message_id = $4, updated_at = to_timestamp($5)
         where id = $1 and bot_id = $2 and chat_id = $3 and lifecycle = 'active'
           and step in ('preview', 'confirming') returning version",
    )
    .bind(dialogue_id)
    .bind(job.bot_id)
    .bind(job.chat_id)
    .bind(message_id)
    .bind(now)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    if let Some(version) = version {
        sqlx::query(
            "update telegram.interaction_tokens set expected_message_id = $2
             where dialogue_id = $1 and expected_dialogue_version = $3 and consumed_at is null",
        )
        .bind(dialogue_id)
        .bind(message_id)
        .bind(version)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    }
    Ok(())
}

async fn apply_binding_ack(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job: &QueuedOutboundJob,
    method: AcknowledgedMethod,
    message_id: i64,
    now: i64,
) -> Result<(), PersistenceError> {
    let Some(operation_id) = job.operation_id else {
        return Ok(());
    };
    if method == AcknowledgedMethod::SendMessage {
        sqlx::query(
            "insert into telegram.message_bindings
                 (id, bot_id, operation_id, chat_id, message_id, updated_at)
             values ($1, $2, $3, $4, $5, to_timestamp($6))
             on conflict (operation_id, chat_id) do update
             set message_id = excluded.message_id, updated_at = excluded.updated_at",
        )
        .bind(sqlx::types::Uuid::now_v7())
        .bind(job.bot_id)
        .bind(operation_id)
        .bind(job.chat_id)
        .bind(message_id)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    }
    if let Some(revision) = job.revision {
        sqlx::query(
            "update telegram.message_bindings
             set last_rendered_revision = $3, last_rendered_at = to_timestamp($4),
                 updated_at = to_timestamp($4)
             where operation_id = $1 and chat_id = $2 and last_rendered_revision < $3",
        )
        .bind(operation_id)
        .bind(job.chat_id)
        .bind(revision)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    }
    Ok(())
}

async fn apply_notification_ack(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job: &QueuedOutboundJob,
    now: i64,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "update telegram.notification_decisions
         set outcome = 'delivered', updated_at = to_timestamp($2)
         where outbound_job_id = $1",
    )
    .bind(job.id)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}
