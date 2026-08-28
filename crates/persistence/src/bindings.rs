//! Identity and chat bindings: the two tables the access gate consults.
//!
//! Identity rows come from the startup bootstrap (the configured owner) and, later, explicit
//! enrollment flows; chat rows appear lazily when an admitted update first mentions a private
//! conversation. `ensure_*` is deliberately insert-if-absent: whatever a row already says about
//! its subject stays authoritative — a re-ensure never refreshes the profile snapshot, never
//! resurrects a disabled principal, and never disturbs a bound Platform user id. The snapshot
//! columns are display evidence; they are never authenticated actor identity.

use sqlx::types::Uuid;

use crate::{Database, PersistenceError};

/// The deployment's access decision for an identity or a chat.
///
/// A closed vocabulary enforced by CHECK constraints; [`AccessState::as_str`] mirrors exactly
/// the strings those constraints accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessState {
    /// The subject is admitted.
    Enabled,
    /// The subject is refused. Recorded rather than deleted so the decision stays auditable.
    Disabled,
}

impl AccessState {
    /// The string stored in the column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    /// The inverse of [`AccessState::as_str`]. Unreachable for data that passed the schema's
    /// CHECK; mapped to a decode failure rather than trusted blindly.
    fn parse(value: &str) -> Result<Self, PersistenceError> {
        match value {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            other => Err(PersistenceError::Query(sqlx::Error::ColumnDecode {
                index: "access_state".to_owned(),
                source: format!("unknown access state `{other}`").into(),
            })),
        }
    }
}

/// Display evidence captured from the Telegram update that first surfaced a sender.
///
/// Written only when the identity row is created; an existing row is never refreshed.
#[derive(Debug, Clone, Default)]
pub struct IdentityProfile {
    /// The sender's @username, if any.
    pub username: Option<String>,
    /// The sender's first name, if any.
    pub first_name: Option<String>,
    /// The sender's last name, if any.
    pub last_name: Option<String>,
}

/// One known Telegram identity, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRecord {
    /// The Telegram user id — the primary key.
    pub telegram_user_id: i64,
    /// The Platform-owned internal user this identity is bound to, once binding lands. An
    /// unenforced reference across schemas; the application owns the invariant.
    pub internal_user_id: Option<Uuid>,
    /// Profile snapshot, first-seen only.
    pub username: Option<String>,
    /// Profile snapshot, first-seen only.
    pub first_name: Option<String>,
    /// Profile snapshot, first-seen only.
    pub last_name: Option<String>,
    /// The deployment's current access decision.
    pub access_state: AccessState,
}

/// One evaluated chat, as stored. This schema version represents private conversations only;
/// the vocabulary is closed at `private`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRecord {
    /// The chat id — the primary key.
    pub chat_id: i64,
    /// The deployment's current access decision.
    pub access_state: AccessState,
}

/// The column tuple an identity read maps from: the deferred Platform binding, the display
/// snapshot, and the closed access vocabulary.
type IdentityRow = (
    Option<Uuid>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

impl Database {
    /// Return the identity for `telegram_user_id`, creating it from `profile` when absent.
    ///
    /// Insert-if-absent: when a row already exists it is returned untouched — the profile
    /// snapshot is not refreshed, a disabled state is not resurrected, and a bound
    /// `internal_user_id` survives. The caller decides nothing here; the returned record is the
    /// authoritative post-write state.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if either statement fails.
    pub async fn ensure_identity(
        &self,
        telegram_user_id: i64,
        profile: &IdentityProfile,
    ) -> Result<IdentityRecord, PersistenceError> {
        // Insert-if-absent, then read back: the read is the authoritative answer whether this
        // call created the row or lost the race against one that did.
        sqlx::query(
            "insert into telegram.identities (telegram_user_id, username, first_name, last_name)
             values ($1, $2, $3, $4)
             on conflict (telegram_user_id) do nothing",
        )
        .bind(telegram_user_id)
        .bind(&profile.username)
        .bind(&profile.first_name)
        .bind(&profile.last_name)
        .execute(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        let row: IdentityRow = sqlx::query_as(
            "select internal_user_id, username, first_name, last_name, access_state
                 from telegram.identities
                 where telegram_user_id = $1",
        )
        .bind(telegram_user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        Ok(IdentityRecord {
            telegram_user_id,
            internal_user_id: row.0,
            username: row.1,
            first_name: row.2,
            last_name: row.3,
            access_state: AccessState::parse(&row.4)?,
        })
    }

    /// The identity for `telegram_user_id`, if the deployment knows it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the query fails.
    pub async fn find_identity(
        &self,
        telegram_user_id: i64,
    ) -> Result<Option<IdentityRecord>, PersistenceError> {
        let row: Option<IdentityRow> = sqlx::query_as(
            "select internal_user_id, username, first_name, last_name, access_state
                 from telegram.identities
                 where telegram_user_id = $1",
        )
        .bind(telegram_user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        row.map(|row| {
            Ok(IdentityRecord {
                telegram_user_id,
                internal_user_id: row.0,
                username: row.1,
                first_name: row.2,
                last_name: row.3,
                access_state: AccessState::parse(&row.4)?,
            })
        })
        .transpose()
    }

    /// Return the chat for `chat_id`, creating it private and enabled when absent.
    ///
    /// Insert-if-absent with the same stability guarantee as [`Database::ensure_identity`]: a
    /// disabled chat stays disabled, and no other column changes behind the caller's back. Only
    /// private conversations reach this method; anything else was denied before a row existed.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if either statement fails.
    pub async fn ensure_chat(&self, chat_id: i64) -> Result<ChatRecord, PersistenceError> {
        // The chat type has no default on purpose: only the gate may create chat rows, and only
        // after deciding the conversation is private.
        sqlx::query(
            "insert into telegram.chats (chat_id, type)
             values ($1, 'private')
             on conflict (chat_id) do nothing",
        )
        .bind(chat_id)
        .execute(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        let access_state: String =
            sqlx::query_scalar("select access_state from telegram.chats where chat_id = $1")
                .bind(chat_id)
                .fetch_one(&self.pool)
                .await
                .map_err(PersistenceError::Query)?;

        Ok(ChatRecord {
            chat_id,
            access_state: AccessState::parse(&access_state)?,
        })
    }

    /// The chat for `chat_id`, if the deployment has evaluated it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the query fails.
    pub async fn find_chat(&self, chat_id: i64) -> Result<Option<ChatRecord>, PersistenceError> {
        let row: Option<String> =
            sqlx::query_scalar("select access_state from telegram.chats where chat_id = $1")
                .bind(chat_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(PersistenceError::Query)?;

        row.map(|access_state| {
            Ok(ChatRecord {
                chat_id,
                access_state: AccessState::parse(&access_state)?,
            })
        })
        .transpose()
    }

    /// Persist the explicit authority that lets `telegram_user_id` receive private notifications
    /// in `chat_id`. The caller invokes this only after the identity and chat access checks pass.
    /// Replaying the same admitted interaction is idempotent; a chat already bound to another
    /// actor is refused by the schema's unique constraint.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the binding does not reference existing admitted records or
    /// conflicts with another actor's binding.
    pub async fn bind_private_chat(
        &self,
        telegram_user_id: i64,
        chat_id: i64,
    ) -> Result<(), PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Query)?;
        sqlx::query(
            "insert into telegram.private_chat_bindings
                 (telegram_user_id, chat_id, bound_at)
             values ($1, $2, now())
             on conflict (telegram_user_id, chat_id) do nothing",
        )
        .bind(telegram_user_id)
        .bind(chat_id)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        sqlx::query(
            "insert into telegram.notification_preferences (telegram_user_id, chat_id)
             values ($1, $2)
             on conflict (telegram_user_id, chat_id) do nothing",
        )
        .bind(telegram_user_id)
        .bind(chat_id)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        transaction.commit().await.map_err(PersistenceError::Query)
    }
}
