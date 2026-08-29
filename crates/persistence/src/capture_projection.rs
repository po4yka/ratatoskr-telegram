//! Atomic local projection of one Platform-accepted capture.

use sqlx::types::Uuid;

use crate::interaction_tokens::NewOperationIntent;
use crate::outbound_jobs::{NewOutboundJob, OutboundJobKind};
use crate::{Database, PersistenceError};

impl Database {
    /// Persist or reconcile the binding, opaque intent, and acknowledgement for one capture.
    ///
    /// The binding row is the serialization authority. Consequently, retries after an unknown
    /// commit and concurrent submissions both converge before checking or creating either of the
    /// other records. Platform HTTP must happen before this method; this transaction is local.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::InvalidCaptureProjection`] if the supplied records disagree, or
    /// [`PersistenceError::Query`] if storage fails. A failed transaction leaves no partial
    /// projection.
    pub async fn record_accepted_capture_projection(
        &self,
        intent: &NewOperationIntent,
        acknowledgement: &NewOutboundJob,
        now: i64,
    ) -> Result<(), PersistenceError> {
        validate_projection(intent, acknowledgement)?;

        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;
        sqlx::query(
            "insert into telegram.message_bindings (id, bot_id, operation_id, chat_id)
             values ($1, $2, $3, $4)
             on conflict (operation_id, chat_id) do nothing",
        )
        .bind(Uuid::now_v7())
        .bind(intent.scope.bot_id)
        .bind(intent.operation_id)
        .bind(intent.scope.chat_id)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;

        let (stored_bot_id, message_id): (i64, Option<i64>) = sqlx::query_as(
            "select bot_id, message_id
             from telegram.message_bindings
             where operation_id = $1 and chat_id = $2
             for update",
        )
        .bind(intent.operation_id)
        .bind(intent.scope.chat_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if stored_bot_id != intent.scope.bot_id {
            return Err(PersistenceError::InvalidCaptureProjection);
        }

        let intent_exists: bool = sqlx::query_scalar(
            "select exists(
                 select 1
                 from telegram.interaction_tokens
                 where surface = 'deep_link' and action = 'operation_status'
                   and bot_id = $1 and telegram_user_id = $2 and chat_id = $3
                   and operation_id = $4 and consumed_at is null
                   and expires_at > to_timestamp($5)
             )",
        )
        .bind(intent.scope.bot_id)
        .bind(intent.scope.telegram_user_id)
        .bind(intent.scope.chat_id)
        .bind(intent.operation_id)
        .bind(now)
        .fetch_one(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if !intent_exists {
            let token = crate::interaction_tokens::mint_token();
            let payload = serde_json::to_value(&intent.payload).map_err(|error| {
                PersistenceError::Query(sqlx::Error::Encode(error.to_string().into()))
            })?;
            sqlx::query(
                "insert into telegram.interaction_tokens
                 (token, surface, action, bot_id, telegram_user_id, chat_id, operation_id, payload,
                  created_at, expires_at)
                 values ($1, 'deep_link', 'operation_status', $2, $3, $4, $5, $6,
                         to_timestamp($7), to_timestamp($8))",
            )
            .bind(token)
            .bind(intent.scope.bot_id)
            .bind(intent.scope.telegram_user_id)
            .bind(intent.scope.chat_id)
            .bind(intent.operation_id)
            .bind(payload)
            .bind(now)
            .bind(intent.expires_at)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
        }

        let acknowledgement_exists = if message_id.is_some() {
            true
        } else {
            sqlx::query_scalar(
                "select exists(
                     select 1
                     from telegram.outbound_jobs
                     where bot_id = $1 and chat_id = $2 and kind = 'send_message'
                       and operation_id = $3 and revision is null
                 )",
            )
            .bind(intent.scope.bot_id)
            .bind(intent.scope.chat_id)
            .bind(intent.operation_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?
        };
        if !acknowledgement_exists {
            crate::outbound_jobs::insert_outbound_job(&mut transaction, acknowledgement, now)
                .await?;
        }

        transaction.commit().await.map_err(PersistenceError::Query)
    }
}

fn validate_projection(
    intent: &NewOperationIntent,
    acknowledgement: &NewOutboundJob,
) -> Result<(), PersistenceError> {
    let consistent = intent.scope.message_id.is_none()
        && acknowledgement.bot_id == intent.scope.bot_id
        && acknowledgement.chat_id == intent.scope.chat_id
        && acknowledgement.kind == OutboundJobKind::SendMessage
        && acknowledgement.operation_id == Some(intent.operation_id)
        && acknowledgement.revision.is_none();
    if consistent {
        Ok(())
    } else {
        Err(PersistenceError::InvalidCaptureProjection)
    }
}
