//! Durable, scoped, optimistic dialogue state.

mod repository_decision;
mod repository_mode;

use repository_mode::{repository_mode, selection_action};

use ratatoskr_github_contracts::{
    GitHubAccountRef, RepositoryActionCapability, RepositoryActionResult,
    RepositoryPreviewResponse, RepositoryPreviewTarget,
};
use sqlx::Row as _;
use sqlx::types::Uuid;

use crate::interaction_tokens::mint_token;
use crate::{Database, PersistenceError};

/// Complete owner scope for a persisted dialogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogueScope {
    /// Bot identity that owns the interaction.
    pub bot_id: i64,
    /// Telegram actor bound to the interaction.
    pub telegram_user_id: i64,
    /// Telegram chat bound to the interaction.
    pub chat_id: i64,
}

/// Closed step vocabulary for the implemented GitHub repository dialogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogueStep {
    /// Awaiting a repository mode selection.
    Preview,
    /// Awaiting explicit confirmation or cancellation.
    Confirming,
    /// Confirmed action is being submitted outside the state transaction.
    Submitting,
    /// Provider outcome has been durably recorded.
    Completed,
}

/// Closed lifecycle vocabulary for every dialogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogueLifecycle {
    /// The dialogue may accept its expected next transition.
    Active,
    /// The intended interaction finished.
    Completed,
    /// The owner cancelled before execution.
    Cancelled,
    /// The timeout boundary ended the interaction.
    Expired,
}

/// Bounded GitHub repository references and selections kept in dialogue state.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubRepositoryDialogue {
    /// Stable preview target, not provider credentials.
    pub target: RepositoryPreviewTarget,
    /// Connected account reference used only when the provider offered `star`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<GitHubAccountRef>,
    /// Explicit selected capability once the preview transition wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_mode: Option<RepositoryActionCapability>,
    /// Closed terminal provider result once submission settles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<RepositoryActionResult>,
}

/// Inputs for a new repository dialogue.
#[derive(Debug, Clone)]
pub struct NewGitHubDialogue {
    /// Complete owner scope.
    pub scope: DialogueScope,
    /// Safe server-side state.
    pub payload: GitHubRepositoryDialogue,
    /// Strict expiry, in whole seconds since the Unix epoch.
    pub expires_at: i64,
}

/// One persisted dialogue as read through its owner scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueRecord {
    /// App-minted stable identity.
    pub id: Uuid,
    /// Complete owner scope.
    pub scope: DialogueScope,
    /// Provider-acknowledged message binding, when stamped.
    pub expected_message_id: Option<i64>,
    /// Current step.
    pub step: DialogueStep,
    /// Monotonic transition version.
    pub version: i64,
    /// Current lifecycle.
    pub lifecycle: DialogueLifecycle,
    /// Safe typed state.
    pub payload: GitHubRepositoryDialogue,
    /// Strict expiry, in whole seconds since the Unix epoch.
    pub expires_at: i64,
}

/// Expected-state transition request for an active dialogue.
#[derive(Debug, Clone, Copy)]
pub struct DialogueTransition {
    /// Dialogue identity.
    pub id: Uuid,
    /// Complete actor scope.
    pub scope: DialogueScope,
    /// Step the caller observed.
    pub expected_step: DialogueStep,
    /// Version the caller observed.
    pub expected_version: i64,
    /// Closed next step.
    pub next_step: DialogueStep,
}

/// Why no dialogue transition was admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogueRefusal {
    /// No dialogue exists under the supplied identity.
    NotFound,
    /// Bot, user, or chat scope did not match.
    ScopeMismatch,
    /// Another transition already changed the expected step or version.
    StaleState,
    /// The dialogue has reached its expiry boundary.
    Expired,
    /// A completed, cancelled, or expired dialogue cannot be revived.
    Terminal,
}

/// How a repository callback was refused without exposing foreign authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackRefusal {
    /// Missing, foreign, malformed, or stale dialogue authority.
    Invalid,
    /// The token or dialogue reached its strict expiry boundary.
    Expired,
    /// A prior presentation consumed the one-time token.
    Consumed,
}

/// One selection button backed by generalized interaction authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionToken {
    /// Capability shown by the button.
    pub mode: RepositoryActionCapability,
    /// Opaque callback data.
    pub token: String,
}

/// Newly persisted preview dialogue and its selection buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewDialogue {
    /// Durable dialogue identity.
    pub dialogue_id: Uuid,
    /// One scoped token for each advertised capability.
    pub selections: Vec<SelectionToken>,
}

/// Selection result and the next pair of one-time decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationTransition {
    /// Durable dialogue identity.
    pub dialogue_id: Uuid,
    /// Explicitly selected capability.
    pub mode: RepositoryActionCapability,
    /// One-time confirmation authority.
    pub confirm_token: String,
    /// One-time cancellation authority.
    pub cancel_token: String,
}

/// Confirmed request facts returned only after the state transaction commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedAction {
    /// Dialogue identity and confirmation evidence reference.
    pub dialogue_id: Uuid,
    /// Explicit selected capability.
    pub mode: RepositoryActionCapability,
    /// Stable repository target.
    pub target: RepositoryPreviewTarget,
    /// Connected account reference for `star` only.
    pub account_ref: Option<GitHubAccountRef>,
    /// Stable action identity fixed at dialogue creation.
    pub idempotency_key: String,
}

/// Result of a terminal confirm/cancel decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionTransition {
    /// Confirmation won and may execute after commit.
    Confirmed(ConfirmedAction),
    /// Cancellation won and no provider action may execute.
    Cancelled,
}

struct LockedCallbackRow {
    dialogue_id: Uuid,
    token_bot_id: i64,
    token_user_id: i64,
    token_chat_id: i64,
    token_message_id: Option<i64>,
    dialogue_bot_id: i64,
    dialogue_user_id: i64,
    dialogue_chat_id: i64,
    dialogue_message_id: Option<i64>,
    payload: serde_json::Value,
    step: String,
    version: i64,
    lifecycle: String,
    idempotency_key: String,
    dialogue_expires_at: i64,
    action: String,
    expected_dialogue_version: i64,
    token_expires_at: i64,
    consumed_at: Option<i64>,
}

impl<'row> sqlx::FromRow<'row, sqlx::postgres::PgRow> for LockedCallbackRow {
    fn from_row(row: &'row sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            dialogue_id: row.try_get("dialogue_id")?,
            token_bot_id: row.try_get("token_bot_id")?,
            token_user_id: row.try_get("token_user_id")?,
            token_chat_id: row.try_get("token_chat_id")?,
            token_message_id: row.try_get("token_message_id")?,
            dialogue_bot_id: row.try_get("dialogue_bot_id")?,
            dialogue_user_id: row.try_get("dialogue_user_id")?,
            dialogue_chat_id: row.try_get("dialogue_chat_id")?,
            dialogue_message_id: row.try_get("dialogue_message_id")?,
            payload: row.try_get("payload")?,
            step: row.try_get("step")?,
            version: row.try_get("version")?,
            lifecycle: row.try_get("lifecycle")?,
            idempotency_key: row.try_get("idempotency_key")?,
            dialogue_expires_at: row.try_get("dialogue_expires_at")?,
            action: row.try_get("action")?,
            expected_dialogue_version: row.try_get("expected_dialogue_version")?,
            token_expires_at: row.try_get("token_expires_at")?,
            consumed_at: row.try_get("consumed_at")?,
        })
    }
}

fn callback_decode_error(index: &str, message: impl Into<String>) -> PersistenceError {
    PersistenceError::Query(sqlx::Error::ColumnDecode {
        index: index.to_owned(),
        source: message.into().into(),
    })
}

fn callback_scope_valid(
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
        && row.dialogue_message_id == Some(message_id)
}

fn validate_callback(
    row: &LockedCallbackRow,
    scope: (i64, i64, i64, i64),
    expected_step: &str,
    expected_version: i64,
    now: i64,
) -> Result<(), CallbackRefusal> {
    if row.consumed_at.is_some() {
        return Err(CallbackRefusal::Consumed);
    }
    if row.token_expires_at <= now || row.dialogue_expires_at <= now {
        return Err(CallbackRefusal::Expired);
    }
    if !callback_scope_valid(row, scope.0, scope.1, scope.2, scope.3)
        || row.lifecycle != "active"
        || row.step != expected_step
        || row.version != expected_version
        || row.expected_dialogue_version != expected_version
    {
        return Err(CallbackRefusal::Invalid);
    }
    Ok(())
}

type DialogueRow = (
    Uuid,
    i64,
    i64,
    i64,
    Option<i64>,
    String,
    i64,
    String,
    serde_json::Value,
    i64,
);

fn step_from_name(value: &str) -> Result<DialogueStep, PersistenceError> {
    match value {
        "preview" => Ok(DialogueStep::Preview),
        "confirming" => Ok(DialogueStep::Confirming),
        "submitting" => Ok(DialogueStep::Submitting),
        "completed" => Ok(DialogueStep::Completed),
        other => Err(dialogue_decode_error("step", other)),
    }
}

const fn step_name(value: DialogueStep) -> &'static str {
    match value {
        DialogueStep::Preview => "preview",
        DialogueStep::Confirming => "confirming",
        DialogueStep::Submitting => "submitting",
        DialogueStep::Completed => "completed",
    }
}

fn lifecycle_from_name(value: &str) -> Result<DialogueLifecycle, PersistenceError> {
    match value {
        "active" => Ok(DialogueLifecycle::Active),
        "completed" => Ok(DialogueLifecycle::Completed),
        "cancelled" => Ok(DialogueLifecycle::Cancelled),
        "expired" => Ok(DialogueLifecycle::Expired),
        other => Err(dialogue_decode_error("lifecycle", other)),
    }
}

fn dialogue_decode_error(index: &str, message: impl Into<String>) -> PersistenceError {
    PersistenceError::Query(sqlx::Error::ColumnDecode {
        index: index.to_owned(),
        source: message.into().into(),
    })
}

fn dialogue_from_row(row: DialogueRow) -> Result<DialogueRecord, PersistenceError> {
    Ok(DialogueRecord {
        id: row.0,
        scope: DialogueScope {
            bot_id: row.1,
            telegram_user_id: row.2,
            chat_id: row.3,
        },
        expected_message_id: row.4,
        step: step_from_name(&row.5)?,
        version: row.6,
        lifecycle: lifecycle_from_name(&row.7)?,
        payload: serde_json::from_value(row.8)
            .map_err(|error| dialogue_decode_error("payload", error.to_string()))?,
        expires_at: row.9,
    })
}

impl Database {
    /// Allocate a repository dialogue identity.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] when storage refuses the new dialogue.
    pub async fn create_github_dialogue(
        &self,
        dialogue: &NewGitHubDialogue,
        now: i64,
    ) -> Result<Uuid, PersistenceError> {
        let id = Uuid::now_v7();
        let payload = serde_json::to_value(&dialogue.payload)
            .map_err(|error| dialogue_decode_error("payload", error.to_string()))?;
        sqlx::query(
            "insert into telegram.dialog_states
             (id, kind, bot_id, telegram_user_id, chat_id, step, version, lifecycle, payload,
              action_idempotency_key, created_at, updated_at, expires_at)
             values ($1, 'github_repository', $2, $3, $4, 'preview', 0, 'active', $5, $6,
                     to_timestamp($7), to_timestamp($7), to_timestamp($8))",
        )
        .bind(id)
        .bind(dialogue.scope.bot_id)
        .bind(dialogue.scope.telegram_user_id)
        .bind(dialogue.scope.chat_id)
        .bind(payload)
        .bind(format!("telegram-github-action.{id}"))
        .bind(now)
        .bind(dialogue.expires_at)
        .execute(self.pool())
        .await
        .map_err(PersistenceError::Query)?;
        Ok(id)
    }

    /// Read one dialogue through the complete owner scope.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] when storage cannot read the dialogue.
    pub async fn find_github_dialogue(
        &self,
        id: Uuid,
        scope: DialogueScope,
    ) -> Result<Option<DialogueRecord>, PersistenceError> {
        let row: Option<DialogueRow> = sqlx::query_as(
            "select id, bot_id, telegram_user_id, chat_id, expected_message_id, step, version,
                    lifecycle, payload, extract(epoch from expires_at)::bigint
             from telegram.dialog_states
             where id = $1 and kind = 'github_repository'
               and bot_id = $2 and telegram_user_id = $3 and chat_id = $4",
        )
        .bind(id)
        .bind(scope.bot_id)
        .bind(scope.telegram_user_id)
        .bind(scope.chat_id)
        .fetch_optional(self.pool())
        .await
        .map_err(PersistenceError::Query)?;
        row.map(dialogue_from_row).transpose()
    }

    /// Advance one expected dialogue step and version.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] when storage cannot apply the transition.
    pub async fn transition_github_dialogue(
        &self,
        transition: DialogueTransition,
        now: i64,
    ) -> Result<Result<DialogueRecord, DialogueRefusal>, PersistenceError> {
        let scope = transition.scope;
        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;
        let expired: Option<Uuid> = sqlx::query_scalar(
            "update telegram.dialog_states
             set lifecycle = 'expired', terminal_at = to_timestamp($5),
                 version = version + 1, updated_at = to_timestamp($5)
             where id = $1 and kind = 'github_repository'
               and bot_id = $2 and telegram_user_id = $3 and chat_id = $4
               and lifecycle = 'active' and expires_at <= to_timestamp($5)
             returning id",
        )
        .bind(transition.id)
        .bind(scope.bot_id)
        .bind(scope.telegram_user_id)
        .bind(scope.chat_id)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if expired.is_some() {
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(Err(DialogueRefusal::Expired));
        }

        let row: Option<DialogueRow> = sqlx::query_as(
            "update telegram.dialog_states
             set step = $6, version = version + 1, updated_at = to_timestamp($7)
             where id = $1 and kind = 'github_repository'
               and bot_id = $2 and telegram_user_id = $3 and chat_id = $4
               and step = $5 and version = $8 and lifecycle = 'active'
               and expires_at > to_timestamp($7)
             returning id, bot_id, telegram_user_id, chat_id, expected_message_id, step, version,
                       lifecycle, payload, extract(epoch from expires_at)::bigint",
        )
        .bind(transition.id)
        .bind(scope.bot_id)
        .bind(scope.telegram_user_id)
        .bind(scope.chat_id)
        .bind(step_name(transition.expected_step))
        .bind(step_name(transition.next_step))
        .bind(now)
        .bind(transition.expected_version)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if let Some(row) = row {
            let record = dialogue_from_row(row)?;
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(Ok(record));
        }

        let current = sqlx::query_as::<_, (i64, i64, i64, String, String, i64)>(
            "select bot_id, telegram_user_id, chat_id, lifecycle, step, version
             from telegram.dialog_states where id = $1 and kind = 'github_repository'",
        )
        .bind(transition.id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        let Some(current) = current else {
            return Ok(Err(DialogueRefusal::NotFound));
        };
        if (current.0, current.1, current.2)
            != (scope.bot_id, scope.telegram_user_id, scope.chat_id)
        {
            return Ok(Err(DialogueRefusal::ScopeMismatch));
        }
        if current.3 != "active" {
            return Ok(Err(DialogueRefusal::Terminal));
        }
        Ok(Err(DialogueRefusal::StaleState))
    }
}

struct NewCallbackAuthority<'a> {
    token: &'a str,
    action: &'a str,
    dialogue_id: Uuid,
    scope: DialogueScope,
    expected_version: i64,
    created_at: i64,
    expires_at: i64,
}

async fn insert_callback_authority(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authority: NewCallbackAuthority<'_>,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "insert into telegram.interaction_tokens
         (token, surface, action, bot_id, telegram_user_id, chat_id, dialogue_id,
          expected_dialogue_version, created_at, expires_at)
         values ($1, 'callback', $2, $3, $4, $5, $6, $7,
                 to_timestamp($8), to_timestamp($9))",
    )
    .bind(authority.token)
    .bind(authority.action)
    .bind(authority.scope.bot_id)
    .bind(authority.scope.telegram_user_id)
    .bind(authority.scope.chat_id)
    .bind(authority.dialogue_id)
    .bind(authority.expected_version)
    .bind(authority.created_at)
    .bind(authority.expires_at)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}

async fn mark_callback_consumed(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    token: &str,
    actor_id: i64,
    now: i64,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "update telegram.interaction_tokens
         set consumed_at = to_timestamp($2), consumed_by_user = $3 where token = $1",
    )
    .bind(token)
    .bind(now)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}

async fn expire_callback_dialogue(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &LockedCallbackRow,
    now: i64,
) -> Result<(), PersistenceError> {
    if row.dialogue_expires_at > now || row.lifecycle != "active" {
        return Ok(());
    }
    sqlx::query(
        "update telegram.dialog_states
         set lifecycle = 'expired', terminal_at = to_timestamp($2), version = version + 1,
             updated_at = to_timestamp($2)
         where id = $1 and lifecycle = 'active'",
    )
    .bind(row.dialogue_id)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}

impl Database {
    /// Persist a repository preview dialogue and one selection token per advertised capability.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if encoding or storage refuses the dialogue.
    pub async fn create_repository_preview_dialogue(
        &self,
        bot_id: i64,
        telegram_user_id: i64,
        chat_id: i64,
        preview: &RepositoryPreviewResponse,
        now: i64,
        expires_at: i64,
    ) -> Result<PreviewDialogue, PersistenceError> {
        let dialogue_id = Uuid::now_v7();
        let scope = DialogueScope {
            bot_id,
            telegram_user_id,
            chat_id,
        };
        let payload = serde_json::to_value(GitHubRepositoryDialogue {
            target: preview.target.clone(),
            account_ref: preview.account_ref.clone(),
            selected_mode: None,
            result: None,
        })
        .map_err(|error| callback_decode_error("payload", error.to_string()))?;
        let idempotency_key = format!("telegram-github-action.{dialogue_id}");
        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;
        sqlx::query(
            "insert into telegram.dialog_states
             (id, kind, bot_id, telegram_user_id, chat_id, step, version, lifecycle, payload,
              action_idempotency_key, created_at, updated_at, expires_at)
             values ($1, 'github_repository', $2, $3, $4, 'preview', 0, 'active', $5, $6,
                     to_timestamp($7), to_timestamp($7), to_timestamp($8))",
        )
        .bind(dialogue_id)
        .bind(bot_id)
        .bind(telegram_user_id)
        .bind(chat_id)
        .bind(payload)
        .bind(idempotency_key)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;

        let mut selections = Vec::with_capacity(preview.available_actions.len());
        for mode in &preview.available_actions {
            let token = mint_token();
            let action = selection_action(*mode);
            insert_callback_authority(
                &mut transaction,
                NewCallbackAuthority {
                    token: &token,
                    action: &action,
                    dialogue_id,
                    scope,
                    expected_version: 0,
                    created_at: now,
                    expires_at,
                },
            )
            .await?;
            selections.push(SelectionToken { mode: *mode, token });
        }
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(PreviewDialogue {
            dialogue_id,
            selections,
        })
    }

    /// Stamp the provider-acknowledged callback message onto the dialogue and current tokens.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if storage cannot stamp the scope.
    pub async fn stamp_callback_message(
        &self,
        dialogue_id: Uuid,
        bot_id: i64,
        chat_id: i64,
        message_id: i64,
        now: i64,
    ) -> Result<bool, PersistenceError> {
        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;
        let version: Option<i64> = sqlx::query_scalar(
            "update telegram.dialog_states
             set expected_message_id = $4, updated_at = to_timestamp($5)
             where id = $1 and bot_id = $2 and chat_id = $3 and lifecycle = 'active'
               and step in ('preview', 'confirming')
             returning version",
        )
        .bind(dialogue_id)
        .bind(bot_id)
        .bind(chat_id)
        .bind(message_id)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        let Some(version) = version else {
            return Ok(false);
        };
        sqlx::query(
            "update telegram.interaction_tokens set expected_message_id = $2
             where dialogue_id = $1 and expected_dialogue_version = $3 and consumed_at is null",
        )
        .bind(dialogue_id)
        .bind(message_id)
        .bind(version)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(true)
    }

    async fn lock_repository_callback(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        token: &str,
    ) -> Result<Option<LockedCallbackRow>, PersistenceError> {
        sqlx::query_as(
            "select d.id as dialogue_id,
                    t.bot_id as token_bot_id, t.telegram_user_id as token_user_id,
                    t.chat_id as token_chat_id, t.expected_message_id as token_message_id,
                    d.bot_id as dialogue_bot_id, d.telegram_user_id as dialogue_user_id,
                    d.chat_id as dialogue_chat_id, d.expected_message_id as dialogue_message_id,
                    d.payload, d.step, d.version, d.lifecycle,
                    d.action_idempotency_key as idempotency_key,
                    extract(epoch from d.expires_at)::bigint as dialogue_expires_at,
                    t.action, t.expected_dialogue_version,
                    extract(epoch from t.expires_at)::bigint as token_expires_at,
                    extract(epoch from t.consumed_at)::bigint as consumed_at
             from telegram.interaction_tokens t
             join telegram.dialog_states d on d.id = t.dialogue_id
             where t.token = $1 and t.surface = 'callback' and d.kind = 'github_repository'
             for update of t, d",
        )
        .bind(token)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)
    }

    /// Consume a selection and mint the expected version-one confirm/cancel pair.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] for storage failures; policy refusals remain values.
    pub async fn consume_repository_selection(
        &self,
        token: &str,
        bot_id: i64,
        actor_id: i64,
        chat_id: i64,
        message_id: i64,
        now: i64,
    ) -> Result<Result<ConfirmationTransition, CallbackRefusal>, PersistenceError> {
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
            "preview",
            0,
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
        let mode = row
            .action
            .strip_prefix("select_")
            .and_then(repository_mode)
            .ok_or_else(|| callback_decode_error("action", "invalid selection"))?;
        let mut payload: GitHubRepositoryDialogue = serde_json::from_value(row.payload.clone())
            .map_err(|error| callback_decode_error("payload", error.to_string()))?;
        payload.selected_mode = Some(mode);
        let payload = serde_json::to_value(payload)
            .map_err(|error| callback_decode_error("payload", error.to_string()))?;
        mark_callback_consumed(&mut transaction, token, actor_id, now).await?;
        sqlx::query(
            "update telegram.dialog_states
             set payload = $2, step = 'confirming', version = 1, expected_message_id = null,
                 updated_at = to_timestamp($3) where id = $1",
        )
        .bind(row.dialogue_id)
        .bind(payload)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        let confirm = mint_token();
        let cancel = mint_token();
        let scope = DialogueScope {
            bot_id,
            telegram_user_id: actor_id,
            chat_id,
        };
        for (value, action) in [(&confirm, "confirm"), (&cancel, "cancel")] {
            insert_callback_authority(
                &mut transaction,
                NewCallbackAuthority {
                    token: value,
                    action,
                    dialogue_id: row.dialogue_id,
                    scope,
                    expected_version: 1,
                    created_at: now,
                    expires_at: row.token_expires_at.min(row.dialogue_expires_at),
                },
            )
            .await?;
        }
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(Ok(ConfirmationTransition {
            dialogue_id: row.dialogue_id,
            mode,
            confirm_token: confirm,
            cancel_token: cancel,
        }))
    }
}
