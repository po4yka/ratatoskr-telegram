//! Terminal confirm/cancel transition and provider-result completion.

use super::*;

/// Durable Telegram update identity authorized to resume one released action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleasingUpdate {
    /// Bot identity that admitted the update.
    pub bot_id: i64,
    /// Bot API update identity under that bot.
    pub update_id: i64,
}

/// Result of atomically completing a released dialogue and enqueueing its Telegram result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionOutcome {
    /// This call committed the completion and result job.
    Completed,
    /// The same releasing update had already committed completion.
    AlreadyCompleted,
}

impl Database {
    /// Consume one version-one confirm/cancel token with a single transactional winner.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] for storage failures; policy refusals remain values.
    pub async fn consume_repository_decision(
        &self,
        token: &str,
        releasing_update: ReleasingUpdate,
        actor_id: i64,
        chat_id: i64,
        message_id: i64,
        now: i64,
    ) -> Result<Result<DecisionTransition, CallbackRefusal>, PersistenceError> {
        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;
        let Some(row) = self
            .lock_repository_callback(&mut transaction, token)
            .await?
        else {
            return Ok(Err(CallbackRefusal::Invalid));
        };
        if row.consumed_at.is_some() {
            return recover_consumed_confirmation(
                &row,
                releasing_update,
                actor_id,
                chat_id,
                message_id,
            );
        }
        if let Err(refusal) = validate_callback(
            &row,
            (releasing_update.bot_id, actor_id, chat_id, message_id),
            "confirming",
            1,
            now,
        ) {
            if refusal == CallbackRefusal::Expired && row.dialogue_expires_at <= now {
                expire_callback_dialogue(&mut transaction, &row, now).await?;
                transaction
                    .commit()
                    .await
                    .map_err(PersistenceError::Query)?;
            }
            return Ok(Err(refusal));
        }
        mark_callback_consumed(&mut transaction, token, actor_id, now).await?;
        if row.action == "cancel" {
            sqlx::query(
                "update telegram.dialog_states
                 set lifecycle = 'cancelled', version = 2, expected_message_id = null,
                     terminal_at = to_timestamp($2), updated_at = to_timestamp($2)
                 where id = $1",
            )
            .bind(row.dialogue_id)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(Ok(DecisionTransition::Cancelled));
        }
        if row.action != "confirm" {
            return Ok(Err(CallbackRefusal::Invalid));
        }
        sqlx::query(
            "update telegram.dialog_states
             set step = 'submitting', version = 2, expected_message_id = null,
                 releasing_bot_id = $3, releasing_update_id = $4,
                 updated_at = to_timestamp($2) where id = $1",
        )
        .bind(row.dialogue_id)
        .bind(now)
        .bind(releasing_update.bot_id)
        .bind(releasing_update.update_id)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        let action = confirmed_action(&row)?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(Ok(DecisionTransition::Confirmed(action)))
    }

    /// Persist the provider result and its Telegram job in one local transaction.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if encoding or storage fails.
    pub async fn complete_repository_dialogue_with_result_job(
        &self,
        dialogue_id: Uuid,
        releasing_update: ReleasingUpdate,
        result: &RepositoryActionResult,
        job: &crate::outbound_jobs::NewOutboundJob,
        now: i64,
    ) -> Result<CompletionOutcome, PersistenceError> {
        let result = Some(
            serde_json::to_value(result)
                .map_err(|error| callback_decode_error("result", error.to_string()))?,
        );
        self.complete_repository_dialogue_projection(
            dialogue_id,
            releasing_update,
            result,
            job,
            now,
        )
        .await
    }

    /// Complete a permanently refused action with one safe Telegram failure job.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if authority validation or storage fails.
    pub async fn complete_repository_dialogue_with_failure_job(
        &self,
        dialogue_id: Uuid,
        releasing_update: ReleasingUpdate,
        job: &crate::outbound_jobs::NewOutboundJob,
        now: i64,
    ) -> Result<CompletionOutcome, PersistenceError> {
        self.complete_repository_dialogue_projection(dialogue_id, releasing_update, None, job, now)
            .await
    }

    async fn complete_repository_dialogue_projection(
        &self,
        dialogue_id: Uuid,
        releasing_update: ReleasingUpdate,
        result: Option<serde_json::Value>,
        job: &crate::outbound_jobs::NewOutboundJob,
        now: i64,
    ) -> Result<CompletionOutcome, PersistenceError> {
        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;
        let current: Option<(String, String, i64, i64)> = sqlx::query_as(
            "select step, lifecycle, bot_id, chat_id
             from telegram.dialog_states
             where id = $1 and releasing_bot_id = $2 and releasing_update_id = $3
             for update",
        )
        .bind(dialogue_id)
        .bind(releasing_update.bot_id)
        .bind(releasing_update.update_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        let Some((step, lifecycle, bot_id, chat_id)) = current else {
            return Err(callback_decode_error(
                "releasing_update",
                "dialogue is not owned by this update",
            ));
        };
        if step == "completed" && lifecycle == "completed" {
            return Ok(CompletionOutcome::AlreadyCompleted);
        }
        let expected_correlation = format!("telegram-dialogue:{dialogue_id}");
        if step != "submitting"
            || lifecycle != "active"
            || job.bot_id != bot_id
            || job.chat_id != chat_id
            || job.kind != crate::outbound_jobs::OutboundJobKind::SendMessage
            || job.operation_id.is_some()
            || job.revision.is_some()
            || job.correlation_id.as_deref() != Some(expected_correlation.as_str())
        {
            return Err(callback_decode_error(
                "dialogue_result",
                "dialogue or result job is inconsistent",
            ));
        }
        sqlx::query(
            "update telegram.dialog_states
             set payload = case when $2::jsonb is null then payload
                                else jsonb_set(payload, '{result}', $2, true) end,
                 step = 'completed',
                 lifecycle = 'completed', version = version + 1,
                 terminal_at = to_timestamp($3), updated_at = to_timestamp($3)
             where id = $1",
        )
        .bind(dialogue_id)
        .bind(result)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        crate::outbound_jobs::insert_outbound_job(&mut transaction, job, now).await?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(CompletionOutcome::Completed)
    }
}

fn recover_consumed_confirmation(
    row: &LockedCallbackRow,
    releasing_update: ReleasingUpdate,
    actor_id: i64,
    chat_id: i64,
    message_id: i64,
) -> Result<Result<DecisionTransition, CallbackRefusal>, PersistenceError> {
    let same_update = row.action == "confirm"
        && recovery_scope_valid(row, releasing_update.bot_id, actor_id, chat_id, message_id)
        && row.releasing_bot_id == Some(releasing_update.bot_id)
        && row.releasing_update_id == Some(releasing_update.update_id);
    if !same_update {
        return Ok(Err(CallbackRefusal::Consumed));
    }
    if row.step == "submitting" && row.lifecycle == "active" {
        return Ok(Ok(DecisionTransition::Confirmed(confirmed_action(row)?)));
    }
    if row.step == "completed" && row.lifecycle == "completed" {
        return Ok(Ok(DecisionTransition::AlreadyCompleted));
    }
    Ok(Err(CallbackRefusal::Consumed))
}

fn recovery_scope_valid(
    row: &LockedCallbackRow,
    bot_id: i64,
    actor_id: i64,
    chat_id: i64,
    message_id: i64,
) -> bool {
    row.token_bot_id == bot_id
        && row.token_user_id == actor_id
        && row.token_chat_id == chat_id
        && row.token_message_id == Some(message_id)
        && row.dialogue_bot_id == bot_id
        && row.dialogue_user_id == actor_id
        && row.dialogue_chat_id == chat_id
}

fn confirmed_action(row: &LockedCallbackRow) -> Result<ConfirmedAction, PersistenceError> {
    let payload: GitHubRepositoryDialogue = serde_json::from_value(row.payload.clone())
        .map_err(|error| callback_decode_error("payload", error.to_string()))?;
    let mode = payload
        .selected_mode
        .ok_or_else(|| callback_decode_error("selected_mode", "missing"))?;
    Ok(ConfirmedAction {
        dialogue_id: row.dialogue_id,
        mode,
        target: payload.target,
        account_ref: if mode == RepositoryActionCapability::Star {
            payload.account_ref
        } else {
            None
        },
        idempotency_key: row.idempotency_key.clone(),
    })
}
