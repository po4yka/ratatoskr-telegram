//! Owner-bound one-time callback authority for GitHub repository confirmation flows.

use base64::Engine as _;
use ratatoskr_github_contracts::{
    GitHubAccountRef, GitHubRepositoryNumericId, GitHubRepositoryUrl, RepositoryActionCapability,
    RepositoryActionResult, RepositoryFullName, RepositoryPreviewResponse, RepositoryPreviewTarget,
};
use sqlx::types::Uuid;

use crate::{Database, PersistenceError};

/// How a callback token was refused without exposing whether another owner's token exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackRefusal {
    /// No usable token authority exists for this presentation.
    Invalid,
    /// The token or its flow has expired.
    Expired,
    /// The one-time transition already lost a race or was consumed.
    Consumed,
}

/// A selection token that can be placed alone in Telegram callback data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionToken {
    /// Mode named only for presentation; authority remains in the stored token row.
    pub mode: RepositoryActionCapability,
    /// Opaque callback data.
    pub token: String,
}

/// A newly persisted preview flow and its available selection authorities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFlow {
    /// Server-side flow reference used by delivery acknowledgment.
    pub flow_id: Uuid,
    /// One opaque token per capability GitHub reported.
    pub selections: Vec<SelectionToken>,
}

/// A valid selection's next confirmation prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationTransition {
    /// Server-side flow reference used by delivery acknowledgment.
    pub flow_id: Uuid,
    /// The exact selected mode.
    pub mode: RepositoryActionCapability,
    /// Opaque one-time confirmation authority.
    pub confirm_token: String,
    /// Opaque one-time cancellation authority.
    pub cancel_token: String,
}

/// A confirmed, durable action identity ready for the network boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedAction {
    /// Server-side flow id, also the confirmation evidence identity.
    pub flow_id: Uuid,
    /// Selected repository mode.
    pub mode: RepositoryActionCapability,
    /// Stable target from the preview.
    pub target: RepositoryPreviewTarget,
    /// Connected account reference, present only when GitHub offered star.
    pub account_ref: Option<GitHubAccountRef>,
    /// Stable retry identity fixed before submission.
    pub idempotency_key: String,
}

/// Result of consuming a terminal decision token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionTransition {
    /// Confirmation won and may submit exactly this action identity.
    Confirmed(ConfirmedAction),
    /// Cancellation won; no action may be submitted.
    Cancelled,
}

type LockedRow = (
    Uuid,
    i64,
    i64,
    i64,
    Option<i64>,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
    String,
    Option<i64>,
);

fn token() -> String {
    let mut bytes = [0_u8; 32];
    bytes
        .get_mut(..16)
        .unwrap_or_default()
        .copy_from_slice(Uuid::now_v7().as_bytes());
    bytes
        .get_mut(16..)
        .unwrap_or_default()
        .copy_from_slice(Uuid::now_v7().as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

const fn mode_name(mode: RepositoryActionCapability) -> &'static str {
    match mode {
        RepositoryActionCapability::Metadata => "metadata",
        RepositoryActionCapability::Track => "track",
        RepositoryActionCapability::Star => "star",
        _ => "unsupported",
    }
}

fn parse_mode(mode: &str) -> Option<RepositoryActionCapability> {
    match mode {
        "metadata" => Some(RepositoryActionCapability::Metadata),
        "track" => Some(RepositoryActionCapability::Track),
        "star" => Some(RepositoryActionCapability::Star),
        _ => None,
    }
}

fn select_action(mode: RepositoryActionCapability) -> &'static str {
    match mode {
        RepositoryActionCapability::Metadata => "select_metadata",
        RepositoryActionCapability::Track => "select_track",
        RepositoryActionCapability::Star => "select_star",
        _ => "unsupported",
    }
}

fn query_error(index: &str, message: impl Into<String>) -> PersistenceError {
    PersistenceError::Query(sqlx::Error::ColumnDecode {
        index: index.to_owned(),
        source: message.into().into(),
    })
}

impl Database {
    /// Persist a preview flow and one opaque selection token for every reported capability.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if storage refuses the flow.
    pub async fn create_repository_preview_flow(
        &self,
        bot_id: i64,
        telegram_user_id: i64,
        chat_id: i64,
        preview: &RepositoryPreviewResponse,
        now: i64,
        expires_at: i64,
    ) -> Result<PreviewFlow, PersistenceError> {
        let flow_id = Uuid::now_v7();
        let idempotency_key = format!("telegram-github-action.{flow_id}");
        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;
        sqlx::query(
            "insert into telegram.callback_flows
             (id, bot_id, telegram_user_id, chat_id, github_repository_numeric_id,
              repository_full_name, canonical_url, account_ref, action_idempotency_key,
              created_at, expires_at, updated_at)
             values ($1,$2,$3,$4,$5,$6,$7,$8,$9,to_timestamp($10),to_timestamp($11),to_timestamp($10))",
        )
        .bind(flow_id)
        .bind(bot_id)
        .bind(telegram_user_id)
        .bind(chat_id)
        .bind(i64::try_from(preview.target.github_repository_numeric_id.get()).map_err(|error| query_error("github_repository_numeric_id", error.to_string()))?)
        .bind(preview.target.repository_full_name.as_str())
        .bind(preview.target.canonical_url.as_str())
        .bind(preview.account_ref.as_ref().map(GitHubAccountRef::as_str))
        .bind(&idempotency_key)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;

        let mut selections = Vec::new();
        for mode in &preview.available_actions {
            let opaque = token();
            sqlx::query(
                "insert into telegram.callback_tokens
                 (token, flow_id, action, expected_version, expires_at)
                 values ($1,$2,$3,0,to_timestamp($4))",
            )
            .bind(&opaque)
            .bind(flow_id)
            .bind(select_action(*mode))
            .bind(expires_at)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
            selections.push(SelectionToken {
                mode: *mode,
                token: opaque,
            });
        }
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(PreviewFlow {
            flow_id,
            selections,
        })
    }

    /// Stamp the Telegram message id only after Bot API acknowledgment.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the flow cannot be updated.
    pub async fn stamp_callback_message(
        &self,
        flow_id: Uuid,
        bot_id: i64,
        chat_id: i64,
        message_id: i64,
        now: i64,
    ) -> Result<bool, PersistenceError> {
        let result = sqlx::query(
            "update telegram.callback_flows set expected_message_id=$4, updated_at=to_timestamp($5)
             where id=$1 and bot_id=$2 and chat_id=$3 and stage in ('preview','confirming')",
        )
        .bind(flow_id)
        .bind(bot_id)
        .bind(chat_id)
        .bind(message_id)
        .bind(now)
        .execute(self.pool())
        .await
        .map_err(PersistenceError::Query)?;
        Ok(result.rows_affected() == 1)
    }

    async fn lock_callback(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        opaque: &str,
    ) -> Result<Option<LockedRow>, PersistenceError> {
        sqlx::query_as(
            "select f.id,f.bot_id,f.telegram_user_id,f.chat_id,f.expected_message_id,
                    f.github_repository_numeric_id,f.repository_full_name,f.canonical_url,
                    f.account_ref,f.mode,f.stage,f.version,extract(epoch from f.expires_at)::bigint,
                    t.action,extract(epoch from t.consumed_at)::bigint
             from telegram.callback_tokens t join telegram.callback_flows f on f.id=t.flow_id
             where t.token=$1 for update of t,f",
        )
        .bind(opaque)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)
    }

    /// Consume one selection and mint distinct confirm/cancel authorities.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] for storage failures; policy refusals remain values.
    pub async fn consume_repository_selection(
        &self,
        opaque: &str,
        bot_id: i64,
        actor_id: i64,
        chat_id: i64,
        message_id: i64,
        now: i64,
    ) -> Result<Result<ConfirmationTransition, CallbackRefusal>, PersistenceError> {
        let mut tx = self.pool().begin().await.map_err(PersistenceError::Query)?;
        let Some(row) = self.lock_callback(&mut tx, opaque).await? else {
            return Ok(Err(CallbackRefusal::Invalid));
        };
        let (
            flow_id,
            row_bot,
            owner,
            row_chat,
            expected,
            _,
            _,
            _,
            _,
            _,
            stage,
            version,
            expires,
            action,
            consumed,
        ) = row;
        if consumed.is_some() {
            return Ok(Err(CallbackRefusal::Consumed));
        }
        if expires <= now {
            return Ok(Err(CallbackRefusal::Expired));
        }
        if row_bot != bot_id
            || owner != actor_id
            || row_chat != chat_id
            || expected != Some(message_id)
            || stage != "preview"
            || version != 0
        {
            return Ok(Err(CallbackRefusal::Invalid));
        }
        let mode = action
            .strip_prefix("select_")
            .and_then(parse_mode)
            .ok_or_else(|| query_error("action", "invalid selection action"))?;
        let confirm = token();
        let cancel = token();
        sqlx::query("update telegram.callback_tokens set consumed_at=to_timestamp($2),consumed_by_user=$3 where token=$1")
            .bind(opaque).bind(now).bind(actor_id).execute(&mut *tx).await.map_err(PersistenceError::Query)?;
        sqlx::query("update telegram.callback_flows set mode=$2,stage='confirming',version=1,expected_message_id=null,updated_at=to_timestamp($3) where id=$1")
            .bind(flow_id).bind(mode_name(mode)).bind(now).execute(&mut *tx).await.map_err(PersistenceError::Query)?;
        for (value, action) in [(&confirm, "confirm"), (&cancel, "cancel")] {
            sqlx::query("insert into telegram.callback_tokens(token,flow_id,action,expected_version,expires_at) values($1,$2,$3,1,to_timestamp($4))")
                .bind(value).bind(flow_id).bind(action).bind(expires).execute(&mut *tx).await.map_err(PersistenceError::Query)?;
        }
        tx.commit().await.map_err(PersistenceError::Query)?;
        Ok(Ok(ConfirmationTransition {
            flow_id,
            mode,
            confirm_token: confirm,
            cancel_token: cancel,
        }))
    }

    /// Consume confirm/cancel with one transactional winner.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] for storage failures; policy refusals remain values.
    pub async fn consume_repository_decision(
        &self,
        opaque: &str,
        bot_id: i64,
        actor_id: i64,
        chat_id: i64,
        message_id: i64,
        now: i64,
    ) -> Result<Result<DecisionTransition, CallbackRefusal>, PersistenceError> {
        let mut tx = self.pool().begin().await.map_err(PersistenceError::Query)?;
        let Some(row) = self.lock_callback(&mut tx, opaque).await? else {
            return Ok(Err(CallbackRefusal::Invalid));
        };
        let (
            flow_id,
            row_bot,
            owner,
            row_chat,
            expected,
            numeric,
            full_name,
            url,
            account,
            mode,
            stage,
            version,
            expires,
            action,
            consumed,
        ) = row;
        if consumed.is_some() {
            return Ok(Err(CallbackRefusal::Consumed));
        }
        if expires <= now {
            return Ok(Err(CallbackRefusal::Expired));
        }
        if row_bot != bot_id
            || owner != actor_id
            || row_chat != chat_id
            || expected != Some(message_id)
            || stage != "confirming"
            || version != 1
        {
            return Ok(Err(CallbackRefusal::Invalid));
        }
        sqlx::query("update telegram.callback_tokens set consumed_at=to_timestamp($2),consumed_by_user=$3 where token=$1")
            .bind(opaque).bind(now).bind(actor_id).execute(&mut *tx).await.map_err(PersistenceError::Query)?;
        if action == "cancel" {
            sqlx::query("update telegram.callback_flows set stage='cancelled',version=2,updated_at=to_timestamp($2) where id=$1")
                .bind(flow_id).bind(now).execute(&mut *tx).await.map_err(PersistenceError::Query)?;
            tx.commit().await.map_err(PersistenceError::Query)?;
            return Ok(Ok(DecisionTransition::Cancelled));
        }
        if action != "confirm" {
            return Ok(Err(CallbackRefusal::Invalid));
        }
        sqlx::query("update telegram.callback_flows set stage='submitting',version=2,updated_at=to_timestamp($2) where id=$1")
            .bind(flow_id).bind(now).execute(&mut *tx).await.map_err(PersistenceError::Query)?;
        tx.commit().await.map_err(PersistenceError::Query)?;
        let mode = mode
            .as_deref()
            .and_then(parse_mode)
            .ok_or_else(|| query_error("mode", "missing mode"))?;
        let target = RepositoryPreviewTarget {
            github_repository_numeric_id: GitHubRepositoryNumericId::new(
                u64::try_from(numeric)
                    .map_err(|error| query_error("numeric", error.to_string()))?,
            )
            .map_err(|error| query_error("numeric", error.to_string()))?,
            repository_full_name: RepositoryFullName::parse(&full_name)
                .map_err(|error| query_error("full_name", error.to_string()))?,
            canonical_url: GitHubRepositoryUrl::parse(&url)
                .map_err(|error| query_error("url", error.to_string()))?,
        };
        Ok(Ok(DecisionTransition::Confirmed(ConfirmedAction {
            flow_id,
            mode,
            target,
            account_ref: if mode == RepositoryActionCapability::Star {
                account
                    .map(|value| GitHubAccountRef::parse(&value))
                    .transpose()
                    .map_err(|error| query_error("account", error.to_string()))?
            } else {
                None
            },
            idempotency_key: format!("telegram-github-action.{flow_id}"),
        })))
    }

    /// Persist GitHub's exact terminal result before any result message is enqueued.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if serialization or storage fails.
    pub async fn complete_repository_flow(
        &self,
        flow_id: Uuid,
        result: &RepositoryActionResult,
        now: i64,
    ) -> Result<(), PersistenceError> {
        let value = serde_json::to_value(result)
            .map_err(|error| query_error("result", error.to_string()))?;
        sqlx::query("update telegram.callback_flows set stage='completed',version=3,result=$2,updated_at=to_timestamp($3) where id=$1 and stage='submitting'")
            .bind(flow_id).bind(value).bind(now).execute(self.pool()).await.map_err(PersistenceError::Query)?;
        Ok(())
    }
}
