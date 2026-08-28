//! Transactional notification preference decisions and outbound admission.

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::outbound_jobs::MessagePayload;
use crate::{Database, PersistenceError};

/// A validated contract notification translated into persistence-owned primitives.
#[derive(Debug, Clone)]
pub struct NewNotificationDelivery {
    /// Bot identity used for the Bot API write.
    pub bot_id: i64,
    /// Transport envelope identity.
    pub event_id: Uuid,
    /// `JetStream` sequence for content-free terminal transport evidence, when available.
    pub stream_sequence: Option<u64>,
    /// Logical contract notification identity.
    pub notification_id: Uuid,
    /// Platform internal user identity from the closed tenant reference.
    pub recipient_user_id: Uuid,
    /// Preserved notification class token.
    pub class: String,
    /// Whether the producer supplied the high-priority advisory hint.
    pub priority_high: bool,
    /// Optional producer quiet-hours offsets from UTC midnight, in seconds.
    pub quiet_hint_seconds: Option<(u32, u32)>,
    /// Privacy-minimal rendered Bot API payload.
    pub payload: MessagePayload,
    /// Opaque correlation reference.
    pub correlation_id: Option<String>,
    /// Producer event instant in Unix seconds, used only for queue aging.
    pub occurred_at: i64,
}

/// One per-chat preference result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDecisionOutcome {
    /// Preference disabled delivery.
    Suppressed,
    /// Quiet hours created one future-due job.
    Deferred,
    /// One immediately due job was created.
    Enqueued,
}

/// Result of one transport message admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationAdmissionResult {
    /// Same envelope event id was already processed.
    DuplicateTransport,
    /// A different event carried a notification already decided for every eligible chat.
    DuplicateNotification,
    /// No explicitly bound, enabled venue exists for the recipient.
    NoEligibleChat,
    /// New decisions in stable chat order.
    Decided(Vec<NotificationDecisionOutcome>),
}

impl Database {
    /// Store content-free evidence for one terminal transport failure.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::InvalidPreference`] for an unknown failure class and
    /// [`PersistenceError::Query`] for a database failure.
    pub async fn record_notification_transport_failure(
        &self,
        stream_sequence: Option<u64>,
        event_id: Option<Uuid>,
        failure_class: &str,
        now: i64,
    ) -> Result<(), PersistenceError> {
        if !matches!(
            failure_class,
            "wrong_event_type"
                | "invalid_envelope"
                | "invalid_notification"
                | "database_unavailable"
        ) {
            return Err(PersistenceError::InvalidPreference);
        }
        let stream_sequence = stream_sequence
            .map(i64::try_from)
            .transpose()
            .map_err(|_| PersistenceError::InvalidPreference)?;
        sqlx::query(
            "insert into telegram.notification_transport_failures
                 (id, stream_sequence, event_id, failure_class, occurred_at)
             values ($1, $2, $3, $4, to_timestamp($5))",
        )
        .bind(Uuid::now_v7())
        .bind(stream_sequence)
        .bind(event_id)
        .bind(failure_class)
        .bind(now)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(PersistenceError::Query)
    }

    /// Apply notification preferences and enqueue delivery atomically.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] when persistence is unavailable.
    #[expect(
        clippy::too_many_lines,
        reason = "the transaction keeps inbox dedupe, policy locks, decisions, jobs and terminal \
                  evidence in their visible commit order"
    )]
    pub async fn admit_notification(
        &self,
        notification: &NewNotificationDelivery,
        now: i64,
    ) -> Result<NotificationAdmissionResult, PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Query)?;
        let transport = sqlx::query(
            "insert into telegram.inbox (event_id, seen_at)
             values ($1, to_timestamp($2)) on conflict (event_id) do nothing",
        )
        .bind(notification.event_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if transport.rows_affected() == 0 {
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(NotificationAdmissionResult::DuplicateTransport);
        }

        let candidates: Vec<CandidateRow> = sqlx::query_as(
            "select identity.telegram_user_id, binding.chat_id, preference.enabled,
                    preference.quiet_policy, preference.quiet_start_minute,
                    preference.quiet_end_minute, preference.high_priority_bypass,
                    coalesce(class_preference.enabled, true)
             from telegram.identities identity
             join telegram.private_chat_bindings binding
               on binding.telegram_user_id = identity.telegram_user_id
             join telegram.chats chat on chat.chat_id = binding.chat_id
             join telegram.notification_preferences preference
               on preference.telegram_user_id = binding.telegram_user_id
              and preference.chat_id = binding.chat_id
             left join telegram.notification_class_preferences class_preference
               on class_preference.telegram_user_id = binding.telegram_user_id
              and class_preference.chat_id = binding.chat_id
              and class_preference.class = $2
             where identity.internal_user_id = $1
               and identity.access_state = 'enabled'
               and chat.access_state = 'enabled'
             order by binding.chat_id
             limit 8
             for update of preference",
        )
        .bind(notification.recipient_user_id)
        .bind(&notification.class)
        .fetch_all(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if candidates.is_empty() {
            let stream_sequence = notification
                .stream_sequence
                .map(i64::try_from)
                .transpose()
                .map_err(|_| PersistenceError::InvalidPreference)?;
            sqlx::query(
                "insert into telegram.notification_transport_failures
                     (id, stream_sequence, event_id, failure_class, occurred_at)
                 values ($1, $2, $3, 'invalid_notification', to_timestamp($4))",
            )
            .bind(Uuid::now_v7())
            .bind(stream_sequence)
            .bind(notification.event_id)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(NotificationAdmissionResult::NoEligibleChat);
        }

        let mut outcomes = Vec::with_capacity(candidates.len());
        let mut inserted_any = false;
        for candidate in candidates {
            let release_at = release_time(&candidate, notification, now)?;
            let outcome = if !candidate.2 || !candidate.7 {
                NotificationDecisionOutcome::Suppressed
            } else if release_at.is_some() {
                NotificationDecisionOutcome::Deferred
            } else {
                NotificationDecisionOutcome::Enqueued
            };
            let decision_id = Uuid::now_v7();
            let inserted = sqlx::query(
                "insert into telegram.notification_decisions
                     (id, notification_id, transport_event_id, chat_id, class, outcome,
                      release_at, decided_at, updated_at)
                 values ($1, $2, $3, $4, $5, $6,
                         case when $7::bigint is null then null else to_timestamp($7) end,
                         to_timestamp($8), to_timestamp($8))
                 on conflict (notification_id, chat_id) do nothing",
            )
            .bind(decision_id)
            .bind(notification.notification_id)
            .bind(notification.event_id)
            .bind(candidate.1)
            .bind(&notification.class)
            .bind(outcome.as_str())
            .bind(release_at)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
            if inserted.rows_affected() == 0 {
                continue;
            }
            inserted_any = true;
            if outcome != NotificationDecisionOutcome::Suppressed {
                let outbound_job_id = insert_notification_job(
                    &mut transaction,
                    notification,
                    candidate.1,
                    release_at.unwrap_or(now),
                    now,
                )
                .await?;
                sqlx::query(
                    "update telegram.notification_decisions
                     set outbound_job_id = $2
                     where id = $1",
                )
                .bind(decision_id)
                .bind(outbound_job_id)
                .execute(&mut *transaction)
                .await
                .map_err(PersistenceError::Query)?;
            }
            outcomes.push(outcome);
        }
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        if inserted_any {
            Ok(NotificationAdmissionResult::Decided(outcomes))
        } else {
            Ok(NotificationAdmissionResult::DuplicateNotification)
        }
    }
}

type CandidateRow = (i64, i64, bool, String, Option<i16>, Option<i16>, bool, bool);

impl NotificationDecisionOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Suppressed => "suppressed",
            Self::Deferred => "deferred",
            Self::Enqueued => "enqueued",
        }
    }
}

fn release_time(
    candidate: &CandidateRow,
    notification: &NewNotificationDelivery,
    now: i64,
) -> Result<Option<i64>, PersistenceError> {
    if !candidate.2 || !candidate.7 || (notification.priority_high && candidate.6) {
        return Ok(None);
    }
    let window = match (candidate.3.as_str(), candidate.4, candidate.5) {
        ("disabled", None, None) => None,
        ("inherit", None, None) => notification.quiet_hint_seconds,
        ("custom", Some(start), Some(end)) => Some((
            u32::try_from(start).map_err(|_| PersistenceError::InvalidPreference)? * 60,
            u32::try_from(end).map_err(|_| PersistenceError::InvalidPreference)? * 60,
        )),
        _ => return Err(PersistenceError::InvalidPreference),
    };
    let Some((start, end)) = window else {
        return Ok(None);
    };
    let second_of_day =
        u32::try_from(now.rem_euclid(86_400)).map_err(|_| PersistenceError::InvalidPreference)?;
    let quiet = if start < end {
        second_of_day >= start && second_of_day < end
    } else {
        second_of_day >= start || second_of_day < end
    };
    if !quiet {
        return Ok(None);
    }
    let seconds_until_end = if second_of_day < end {
        end - second_of_day
    } else {
        86_400 - second_of_day + end
    };
    Ok(Some(now + i64::from(seconds_until_end)))
}

async fn insert_notification_job(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    notification: &NewNotificationDelivery,
    chat_id: i64,
    due_at: i64,
    now: i64,
) -> Result<Uuid, PersistenceError> {
    let payload = notification.payload.canonical()?;
    let content_hash = format!("{:x}", Sha256::digest(payload.as_bytes()));
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into telegram.outbound_jobs
             (id, bot_id, chat_id, kind, payload, content_hash, correlation_id,
              delivery_class, notification_id, notification_created_at,
              next_attempt_at, created_at, updated_at)
         values ($1, $2, $3, 'send_message', $4::jsonb, $5, $6, 'notification', $7,
                 to_timestamp($8), to_timestamp($9), to_timestamp($10), to_timestamp($10))",
    )
    .bind(id)
    .bind(notification.bot_id)
    .bind(chat_id)
    .bind(payload)
    .bind(content_hash)
    .bind(&notification.correlation_id)
    .bind(notification.notification_id)
    .bind(notification.occurred_at)
    .bind(due_at)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map(|_| id)
    .map_err(PersistenceError::Query)
}
