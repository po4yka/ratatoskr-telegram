//! Telegram-owned notification preferences and optimistic updates.

use std::collections::BTreeMap;

use crate::{Database, PersistenceError};

/// Which quiet-hours source a user selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietPolicy {
    /// Never defer for quiet hours.
    Disabled,
    /// Apply the producer hint when one is present.
    Inherit,
    /// Apply the user's own UTC-minute window.
    Custom {
        /// Inclusive start minute after UTC midnight.
        start_minute: u16,
        /// Exclusive end minute after UTC midnight.
        end_minute: u16,
    },
}

impl QuietPolicy {
    fn columns(self) -> Result<(&'static str, Option<i16>, Option<i16>), PersistenceError> {
        match self {
            Self::Disabled => Ok(("disabled", None, None)),
            Self::Inherit => Ok(("inherit", None, None)),
            Self::Custom {
                start_minute,
                end_minute,
            } if start_minute < 1_440 && end_minute < 1_440 && start_minute != end_minute => Ok((
                "custom",
                Some(i16::try_from(start_minute).map_err(|_| PersistenceError::InvalidPreference)?),
                Some(i16::try_from(end_minute).map_err(|_| PersistenceError::InvalidPreference)?),
            )),
            Self::Custom { .. } => Err(PersistenceError::InvalidPreference),
        }
    }

    fn from_columns(
        policy: &str,
        start: Option<i16>,
        end: Option<i16>,
    ) -> Result<Self, PersistenceError> {
        match (policy, start, end) {
            ("disabled", None, None) => Ok(Self::Disabled),
            ("inherit", None, None) => Ok(Self::Inherit),
            ("custom", Some(start), Some(end)) => Ok(Self::Custom {
                start_minute: u16::try_from(start)
                    .map_err(|_| PersistenceError::InvalidPreference)?,
                end_minute: u16::try_from(end).map_err(|_| PersistenceError::InvalidPreference)?,
            }),
            _ => Err(PersistenceError::InvalidPreference),
        }
    }
}

/// One complete private-chat preference snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPreferences {
    /// Telegram actor owning the preference.
    pub telegram_user_id: i64,
    /// Explicitly bound private chat.
    pub chat_id: i64,
    /// Global notification toggle.
    pub enabled: bool,
    /// Selected quiet-hours behavior.
    pub quiet_policy: QuietPolicy,
    /// Whether a producer's high-priority hint may bypass quiet hours.
    pub high_priority_bypass: bool,
    /// Optimistic version.
    pub version: i64,
    class_overrides: BTreeMap<String, bool>,
}

impl NotificationPreferences {
    /// Return the explicit toggle for `class`, if one exists.
    #[must_use]
    pub fn class_enabled(&self, class: &str) -> Option<bool> {
        self.class_overrides.get(class).copied()
    }
}

/// Atomic preference mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPreferenceUpdate {
    /// New global toggle.
    pub enabled: bool,
    /// New quiet-hours behavior.
    pub quiet_policy: QuietPolicy,
    /// New high-priority bypass choice.
    pub high_priority_bypass: bool,
    /// One class override to upsert or remove (`None` means inherit); unrelated overrides remain
    /// untouched.
    pub class_override: Option<(String, Option<bool>)>,
}

type PreferenceRow = (bool, String, Option<i16>, Option<i16>, bool, i64);

impl Database {
    /// Read one complete preference snapshot for an explicit actor/chat binding.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the database read fails, or
    /// [`PersistenceError::InvalidPreference`] if stored state violates the closed model.
    pub async fn notification_preferences(
        &self,
        telegram_user_id: i64,
        chat_id: i64,
    ) -> Result<Option<NotificationPreferences>, PersistenceError> {
        let row: Option<PreferenceRow> = sqlx::query_as(
            "select enabled, quiet_policy, quiet_start_minute, quiet_end_minute,
                    high_priority_bypass, version
             from telegram.notification_preferences
             where telegram_user_id = $1 and chat_id = $2",
        )
        .bind(telegram_user_id)
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;
        let Some(row) = row else { return Ok(None) };
        let overrides: Vec<(String, bool)> = sqlx::query_as(
            "select class, enabled from telegram.notification_class_preferences
             where telegram_user_id = $1 and chat_id = $2 order by class",
        )
        .bind(telegram_user_id)
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;
        Ok(Some(NotificationPreferences {
            telegram_user_id,
            chat_id,
            enabled: row.0,
            quiet_policy: QuietPolicy::from_columns(&row.1, row.2, row.3)?,
            high_priority_bypass: row.4,
            version: row.5,
            class_overrides: overrides.into_iter().collect(),
        }))
    }

    /// Atomically update one preference snapshot if `expected_version` is current.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::InvalidPreference`] for malformed windows/classes,
    /// [`PersistenceError::StalePreference`] for absent or stale rows, and
    /// [`PersistenceError::Query`] for database failures.
    pub async fn update_notification_preferences(
        &self,
        telegram_user_id: i64,
        chat_id: i64,
        expected_version: i64,
        update: &NotificationPreferenceUpdate,
        now: i64,
    ) -> Result<NotificationPreferences, PersistenceError> {
        let (policy, start, end) = update.quiet_policy.columns()?;
        if update
            .class_override
            .as_ref()
            .is_some_and(|(class, _)| !valid_class(class))
        {
            return Err(PersistenceError::InvalidPreference);
        }
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Query)?;
        let changed = sqlx::query(
            "update telegram.notification_preferences
             set enabled = $4, quiet_policy = $5, quiet_start_minute = $6,
                 quiet_end_minute = $7, high_priority_bypass = $8,
                 version = version + 1, updated_at = to_timestamp($9)
             where telegram_user_id = $1 and chat_id = $2 and version = $3",
        )
        .bind(telegram_user_id)
        .bind(chat_id)
        .bind(expected_version)
        .bind(update.enabled)
        .bind(policy)
        .bind(start)
        .bind(end)
        .bind(update.high_priority_bypass)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if changed.rows_affected() != 1 {
            transaction
                .rollback()
                .await
                .map_err(PersistenceError::Query)?;
            return Err(PersistenceError::StalePreference);
        }
        if let Some((class, enabled)) = &update.class_override {
            if let Some(enabled) = enabled {
                sqlx::query(
                    "insert into telegram.notification_class_preferences
                         (telegram_user_id, chat_id, class, enabled, updated_at)
                     values ($1, $2, $3, $4, to_timestamp($5))
                     on conflict (telegram_user_id, chat_id, class) do update
                     set enabled = excluded.enabled, updated_at = excluded.updated_at",
                )
                .bind(telegram_user_id)
                .bind(chat_id)
                .bind(class)
                .bind(enabled)
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(PersistenceError::Query)?;
            } else {
                sqlx::query(
                    "delete from telegram.notification_class_preferences
                     where telegram_user_id = $1 and chat_id = $2 and class = $3",
                )
                .bind(telegram_user_id)
                .bind(chat_id)
                .bind(class)
                .execute(&mut *transaction)
                .await
                .map_err(PersistenceError::Query)?;
            }
        }
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        self.notification_preferences(telegram_user_id, chat_id)
            .await?
            .ok_or(PersistenceError::StalePreference)
    }
}

fn valid_class(class: &str) -> bool {
    !class.is_empty()
        && class.len() <= 32
        && class.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_lowercase()
            } else {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            }
        })
}
