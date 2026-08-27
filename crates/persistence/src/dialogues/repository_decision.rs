//! Terminal confirm/cancel transition and provider-result completion.

use super::*;

impl Database {
    /// Consume one version-one confirm/cancel token with a single transactional winner.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] for storage failures; policy refusals remain values.
    pub async fn consume_repository_decision(
        &self,
        token: &str,
        bot_id: i64,
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
        if let Err(refusal) = validate_callback(
            &row,
            (bot_id, actor_id, chat_id, message_id),
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
                 updated_at = to_timestamp($2) where id = $1",
        )
        .bind(row.dialogue_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        let payload: GitHubRepositoryDialogue = serde_json::from_value(row.payload)
            .map_err(|error| callback_decode_error("payload", error.to_string()))?;
        let mode = payload
            .selected_mode
            .ok_or_else(|| callback_decode_error("selected_mode", "missing"))?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(Ok(DecisionTransition::Confirmed(ConfirmedAction {
            dialogue_id: row.dialogue_id,
            mode,
            target: payload.target,
            account_ref: if mode == RepositoryActionCapability::Star {
                payload.account_ref
            } else {
                None
            },
            idempotency_key: row.idempotency_key,
        })))
    }

    /// Persist the exact provider result and complete a submitting repository dialogue.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if encoding or storage fails.
    pub async fn complete_repository_dialogue(
        &self,
        dialogue_id: Uuid,
        result: &RepositoryActionResult,
        now: i64,
    ) -> Result<(), PersistenceError> {
        let result = serde_json::to_value(result)
            .map_err(|error| callback_decode_error("result", error.to_string()))?;
        let changed = sqlx::query(
            "update telegram.dialog_states
             set payload = jsonb_set(payload, '{result}', $2, true), step = 'completed',
                 lifecycle = 'completed', version = version + 1,
                 terminal_at = to_timestamp($3), updated_at = to_timestamp($3)
             where id = $1 and step = 'submitting' and lifecycle = 'active'",
        )
        .bind(dialogue_id)
        .bind(result)
        .bind(now)
        .execute(self.pool())
        .await
        .map_err(PersistenceError::Query)?;
        if changed.rows_affected() != 1 {
            return Err(callback_decode_error(
                "dialogue_id",
                "dialogue is not submitting",
            ));
        }
        Ok(())
    }
}
