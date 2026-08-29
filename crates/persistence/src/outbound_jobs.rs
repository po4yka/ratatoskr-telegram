//! The durable outbound queue: enqueue, per-chat FIFO claim with leases, supersede sweeps, and
//! settlement.
//!
//! Every Bot API write is a row here before any network call, so a crash between acceptance and
//! delivery loses nothing. Three decisions are owned by this module rather than its callers:
//! attempts increment AT CLAIM (a crash mid-send leaves the count advanced, which keeps the
//! retry bound honest across restarts — counting at settle would let a crashed sender retry
//! forever), claiming is strict FIFO per chat with at most one job in flight per chat (conflicting
//! edits become impossible by construction), and superseding is an enqueue-time sweep over
//! still-waiting jobs only (in-flight work is settled by the sender's own revision check).
//!
//! Every timestamp is caller-supplied (whole seconds since the Unix epoch) and converted at the
//! SQL boundary with `to_timestamp`, so eligibility, leases, and backoff stay deterministic under
//! an injected clock; no query reads the database clock.

use sqlx::types::Uuid;
use sqlx::{Postgres, Transaction};

use crate::{Database, PersistenceError};

/// What kind of Bot API write a job performs. A closed vocabulary enforced by the schema's CHECK;
/// [`OutboundJobKind::as_str`] mirrors exactly the strings it accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundJobKind {
    /// `sendMessage`: deliver a fresh message to the chat.
    SendMessage,
    /// `editMessageText`: rewrite the text of an already-bound message.
    EditMessageText,
}

impl OutboundJobKind {
    /// The string stored in the column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SendMessage => "send_message",
            Self::EditMessageText => "edit_message_text",
        }
    }

    /// The inverse of [`OutboundJobKind::as_str`]. Unreachable for data that passed the schema's
    /// CHECK; mapped to a decode failure rather than trusted blindly.
    fn parse(value: &str) -> Result<Self, PersistenceError> {
        match value {
            "send_message" => Ok(Self::SendMessage),
            "edit_message_text" => Ok(Self::EditMessageText),
            other => Err(PersistenceError::Query(sqlx::Error::ColumnDecode {
                index: "kind".to_owned(),
                source: format!("unknown outbound job kind `{other}`").into(),
            })),
        }
    }
}

/// Where a job sits in its lifecycle. The tokens are ARCHITECTURE.md §18.1's exact vocabulary —
/// closed by the schema's CHECK, mirrored here, never renamed locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundJobState {
    /// Written by a producer that defers scheduling; not yet eligible for claims.
    Planned,
    /// Eligible for claiming from `next_attempt_at` onward. The insert default.
    Ready,
    /// Claimed by a sender and inside its lease window.
    Sending,
    /// Delivered (or answered `message is not modified`, which is success).
    Sent,
    /// Waiting out a backoff or rate-limit delay before becoming ready again.
    RetryWait,
    /// A newer revision replaced this job before delivery; it will never run.
    Superseded,
    /// Dead-lettered: permanently undeliverable, or transient failures exhausted the bound.
    FailedPermanent,
    /// Telegram may have applied a non-idempotent send, so automatic replay is forbidden.
    OutcomeUnknown,
    /// Withdrawn by its producer before delivery.
    Cancelled,
}

impl OutboundJobState {
    /// The string stored in the column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Ready => "ready",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::RetryWait => "retry_wait",
            Self::Superseded => "superseded",
            Self::FailedPermanent => "failed_permanent",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::Cancelled => "cancelled",
        }
    }

    /// The inverse of [`OutboundJobState::as_str`]. Unreachable for data that passed the schema's
    /// CHECK; mapped to a decode failure rather than trusted blindly.
    #[expect(
        dead_code,
        reason = "the parse side completes the mirrored vocabulary pair; the first reader arrives \
                  with the sender worker later in this change"
    )]
    fn parse(value: &str) -> Result<Self, PersistenceError> {
        match value {
            "planned" => Ok(Self::Planned),
            "ready" => Ok(Self::Ready),
            "sending" => Ok(Self::Sending),
            "sent" => Ok(Self::Sent),
            "retry_wait" => Ok(Self::RetryWait),
            "superseded" => Ok(Self::Superseded),
            "failed_permanent" => Ok(Self::FailedPermanent),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(PersistenceError::Query(sqlx::Error::ColumnDecode {
                index: "state".to_owned(),
                source: format!("unknown outbound job state `{other}`").into(),
            })),
        }
    }
}

/// The whole rendered message a job delivers: text plus the optional presentation the Bot API
/// needs to reproduce it exactly - parse mode and inline keyboard when the render carries one.
/// Serialized as one jsonb column, so markup survives queueing, restarts, and retries
/// bit-identically; identical re-render detection hashes this canonical serialization.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MessagePayload {
    /// The message text, already escaped/composed by the renderer for its parse mode.
    pub text: String,
    /// The parse mode label (`HTML`) when the text carries markup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
    /// The inline keyboard layout when buttons ride along; the wire shape the Bot API expects
    /// under `reply_markup`, kept value-typed so persistence stays free of client types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<serde_json::Value>,
}

impl MessagePayload {
    /// A plain-text payload with no markup.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            parse_mode: None,
            reply_markup: None,
        }
    }

    /// The canonical serialization the content hash covers: field order fixed by the struct,
    /// absent options omitted.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if serialization fails, which would be a broken build rather
    /// than bad input.
    pub fn canonical(&self) -> Result<String, PersistenceError> {
        serde_json::to_string(self).map_err(|error| {
            PersistenceError::Query(sqlx::Error::ColumnDecode {
                index: "payload".to_owned(),
                source: error.to_string().into(),
            })
        })
    }

    fn from_stored(raw: &str) -> Result<Self, PersistenceError> {
        serde_json::from_str(raw).map_err(|error| {
            PersistenceError::Query(sqlx::Error::ColumnDecode {
                index: "payload".to_owned(),
                source: error.to_string().into(),
            })
        })
    }
}

/// A job to enqueue. The caller renders the payload and computes `content_hash` over the
/// canonical payload serialization — persistence stores what it is given and hashes nothing
/// itself, so the hash always matches the exact bytes the sender will put on the wire.
#[derive(Debug, Clone)]
pub struct NewOutboundJob {
    /// The bot that will perform the write.
    pub bot_id: i64,
    /// The chat the write targets.
    pub chat_id: i64,
    /// Which Bot API method to call.
    pub kind: OutboundJobKind,
    /// The whole rendered message.
    pub payload: MessagePayload,
    /// sha256 hex of the canonical [`Self::payload`] serialization, computed by the caller;
    /// identical-render no-op detection.
    pub content_hash: String,
    /// The operation this job belongs to or references, when one exists.
    pub operation_id: Option<Uuid>,
    /// The projection revision this job renders; meaningful for edits and drives superseding.
    pub revision: Option<i64>,
    /// The contracts `EntityRef` correlation string, carried for tracing only.
    pub correlation_id: Option<String>,
    /// Earliest attempt time, whole seconds since the Unix epoch. `None` means immediately —
    /// the column default stamps the database's `now()`.
    pub next_attempt_at: Option<i64>,
}

/// One claimed job, carrying everything the Bot API call needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedOutboundJob {
    /// The job's id.
    pub id: Uuid,
    /// The bot that will perform the write.
    pub bot_id: i64,
    /// The chat the write targets.
    pub chat_id: i64,
    /// Which Bot API method to call.
    pub kind: OutboundJobKind,
    /// The whole rendered message.
    pub payload: MessagePayload,
    /// sha256 hex of the canonical [`Self::payload`] serialization.
    pub content_hash: String,
    /// The operation this job belongs to or references, when one exists.
    pub operation_id: Option<Uuid>,
    /// The projection revision this job renders.
    pub revision: Option<i64>,
    /// The contracts `EntityRef` correlation string, carried for tracing only.
    pub correlation_id: Option<String>,
    /// Original quarantined attempt when an operator explicitly authorized this new attempt.
    pub recovery_of: Option<Uuid>,
    /// Notification class for notification jobs; absent for direct/projection work.
    pub notification_class: Option<String>,
    /// How many times this job has been claimed, including this one.
    pub attempts: i32,
}

/// What the sender learned when a claimed job left its hands. One variant per settlement branch;
/// mapping it onto state transitions lives entirely in [`Database::settle_outbound_job`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// The Bot API acknowledged the write.
    Sent,
    /// The sender found the job's revision not newer than the binding's last rendered one; the
    /// job is withdrawn without an API call.
    SupersededStale,
    /// The Bot API answered `message is not modified`; treated as success.
    NotModified,
    /// A transient failure: wait `delay_secs` and try again — unless the attempt bound is
    /// already reached, in which case the job dead-letters.
    RetryWithBackoff {
        /// Whole seconds to wait before the next attempt.
        delay_secs: u32,
    },
    /// A permanent failure classified as `class` (a closed safe label, never provider text);
    /// dead-letters immediately.
    FailedPermanent {
        /// The safe failure-class label recorded on the row.
        class: String,
    },
    /// A non-idempotent write may have reached Telegram and cannot be reconciled safely.
    OutcomeUnknown {
        /// Closed, content-free uncertainty class.
        class: String,
    },
}

/// The exact Bot API method whose successful acknowledgement is being recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcknowledgedMethod {
    /// A fresh, non-idempotent `sendMessage`.
    SendMessage,
    /// An idempotent `editMessageText`.
    EditMessageText,
}

impl AcknowledgedMethod {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SendMessage => "send_message",
            Self::EditMessageText => "edit_message_text",
        }
    }
}

/// The column tuple a claimed job maps from, in claim-SQL order. Kind crosses as its stored
/// string and is parsed back through the closed vocabulary.
type ClaimedRow = (
    Uuid,
    i64,
    i64,
    String,
    String,
    String,
    Option<Uuid>,
    Option<i64>,
    Option<String>,
    Option<Uuid>,
    Option<String>,
    i32,
);

impl Database {
    /// Insert a job as `ready` and return its minted id.
    ///
    /// When the job carries both an operation reference and a revision, older still-waiting edit
    /// jobs of the same binding are swept to `superseded` in the same transaction — a newer
    /// render makes every older queued render pointless, and sweeping at enqueue keeps the due
    /// scan small. In-flight (`sending`) and terminal states are untouched: what is already on
    /// the wire settles on its own facts. The id is minted here (`UUIDv7`) because the schema gives
    /// id columns no DEFAULT; v7 sorts by time, so id order within a chat approximates enqueue
    /// order and the FIFO claim needs no separate sequence.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if any statement fails.
    pub async fn enqueue_outbound_job(
        &self,
        job: &NewOutboundJob,
        now: i64,
    ) -> Result<Uuid, PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Query)?;
        let id = insert_outbound_job(&mut transaction, job, now).await?;

        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(id)
    }

    /// Claim the next due job, honoring strict per-chat FIFO and one-job-in-flight-per-chat.
    ///
    /// Eligible means waiting (`ready`/`retry_wait`) past its `next_attempt_at`, or `sending`
    /// past its lease expiry (a crashed sender's orphan). Among eligible rows each chat's head —
    /// its lowest id, which under `UUIDv7` minting is its earliest enqueue — competes, and ONE head
    /// wins per call. The winner flips to `sending` with a fresh lease and its attempt count
    /// advances AT CLAIM: a crash between send and settlement leaves the count honest, which is
    /// what bounds retries across restarts.
    ///
    /// One statement does the whole dance. `PostgreSQL` forbids `FOR UPDATE` alongside `DISTINCT
    /// ON`, so the candidate rows are locked first (`SKIP LOCKED`, so concurrent senders never
    /// block each other) and the per-chat head is picked from the locked set — the same shape the
    /// update claim in `updates.rs` uses.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails.
    pub async fn claim_due_outbound_job(
        &self,
        now: i64,
        lease_ttl_secs: u32,
    ) -> Result<Option<QueuedOutboundJob>, PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Query)?;
        quarantine_expired_sends(&mut transaction, now).await?;
        let row: Option<ClaimedRow> = sqlx::query_as(
            "with busy as (
                 select distinct chat_id
                 from telegram.outbound_jobs
                 where state = 'sending' and lease_expires_at > to_timestamp($1)
             ),
             candidates as (
                 select id, chat_id
                 from telegram.outbound_jobs
                 where chat_id not in (select chat_id from busy)
                   and ((state in ('ready', 'retry_wait')
                            and next_attempt_at <= to_timestamp($1))
                        or (state = 'sending' and lease_expires_at <= to_timestamp($1)))
                 order by chat_id, id
                 for update skip locked
             ),
             ranked as (
                 select id, chat_id,
                        case
                          when delivery_class = 'notification'
                           and notification_created_at <= to_timestamp($1 - 300) then 0
                          when delivery_class = 'direct' then 1
                          when delivery_class = 'projection' then 2
                          else 3
                        end as delivery_rank
                 from candidates
                 join telegram.outbound_jobs using (id, chat_id)
             ),
             chat_heads as (
                 select distinct on (chat_id) id, delivery_rank
                 from ranked
                 order by chat_id, delivery_rank, id
             ),
             head as (
                 select id from chat_heads
                 order by delivery_rank, id
                 limit 1
             ),
             claimed as (
             update telegram.outbound_jobs as job
                 set state = 'sending',
                     lease_expires_at = to_timestamp($1 + $2),
                     attempts = job.attempts + 1,
                 updated_at = to_timestamp($1)
             from head
             where job.id = head.id
              returning job.id, job.bot_id, job.chat_id, job.kind, job.payload::text,
                        job.content_hash, job.operation_id, job.revision, job.correlation_id,
                        job.recovery_of, job.notification_id, job.attempts
             )
             select claimed.id, claimed.bot_id, claimed.chat_id, claimed.kind, claimed.payload,
                    claimed.content_hash, claimed.operation_id, claimed.revision,
                    claimed.correlation_id, claimed.recovery_of, decision.class, claimed.attempts
             from claimed
             left join telegram.notification_decisions decision
               on decision.outbound_job_id = claimed.id",
        )
        .bind(now)
        .bind(i64::from(lease_ttl_secs))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;

        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;

        row.map(|claimed| {
            Ok(QueuedOutboundJob {
                id: claimed.0,
                bot_id: claimed.1,
                chat_id: claimed.2,
                kind: OutboundJobKind::parse(&claimed.3)?,
                payload: MessagePayload::from_stored(&claimed.4)?,
                content_hash: claimed.5,
                operation_id: claimed.6,
                revision: claimed.7,
                correlation_id: claimed.8,
                recovery_of: claimed.9,
                notification_class: claimed.10,
                attempts: claimed.11,
            })
        })
        .transpose()
    }

    /// Settle a claimed job according to the outcome its delivery produced.
    ///
    /// Successes (`Sent`, `NotModified`) end `sent`; `SupersededStale` ends `superseded`;
    /// `FailedPermanent` dead-letters immediately recording its class; `RetryWithBackoff`
    /// reschedules at `now + delay` — unless the job has already consumed `max_attempts`, in
    /// which case it dead-letters with the `transient` class instead of looping forever. Any
    /// settlement clears the lease. The retry bound is evaluated against the row's attempt count
    /// INSIDE the statement, so a concurrent reclaim cannot slip past the bound.
    ///
    /// The sender presents the attempt number it received at claim time. A reclaimed worker's
    /// stale settlement is rejected, so it cannot overwrite the newer lease holder's outcome.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::UnknownOutboundJob`] when no row carries `id`;
    /// [`PersistenceError::StaleOutboundClaim`] when `id` no longer belongs to this attempt;
    /// [`PersistenceError::Query`] if the statement fails otherwise.
    pub async fn settle_outbound_job(
        &self,
        id: Uuid,
        claim_attempt: i32,
        now: i64,
        max_attempts: i32,
        outcome: &DeliveryOutcome,
    ) -> Result<(), PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Query)?;
        let result = match outcome {
            DeliveryOutcome::Sent | DeliveryOutcome::NotModified => {
                settle_terminal(
                    &mut transaction,
                    id,
                    claim_attempt,
                    now,
                    OutboundJobState::Sent,
                    None,
                )
                .await?
            }
            DeliveryOutcome::SupersededStale => {
                settle_terminal(
                    &mut transaction,
                    id,
                    claim_attempt,
                    now,
                    OutboundJobState::Superseded,
                    None,
                )
                .await?
            }
            DeliveryOutcome::FailedPermanent { class } => {
                settle_terminal(
                    &mut transaction,
                    id,
                    claim_attempt,
                    now,
                    OutboundJobState::FailedPermanent,
                    Some(class),
                )
                .await?
            }
            DeliveryOutcome::OutcomeUnknown { class } => {
                settle_terminal(
                    &mut transaction,
                    id,
                    claim_attempt,
                    now,
                    OutboundJobState::OutcomeUnknown,
                    Some(class),
                )
                .await?
            }
            DeliveryOutcome::RetryWithBackoff { delay_secs } => sqlx::query(
                "update telegram.outbound_jobs
                     set state = case when attempts >= $2
                                      then 'failed_permanent' else 'retry_wait' end,
                         last_error_class = case when attempts >= $2
                                                 then 'transient' else last_error_class end,
                         next_attempt_at = case when attempts >= $2
                                                then next_attempt_at
                                                else to_timestamp($3 + $4) end,
                         lease_expires_at = null,
                         updated_at = to_timestamp($3)
                     where id = $1 and state = 'sending' and attempts = $5",
            )
            .bind(id)
            .bind(max_attempts)
            .bind(now)
            .bind(i64::from(*delay_secs))
            .bind(claim_attempt)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?,
        };

        if result.rows_affected() == 0 {
            let exists: bool = sqlx::query_scalar(
                "select exists(select 1 from telegram.outbound_jobs where id = $1)",
            )
            .bind(id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
            transaction
                .rollback()
                .await
                .map_err(PersistenceError::Query)?;
            return Err(if exists {
                PersistenceError::StaleOutboundClaim
            } else {
                PersistenceError::UnknownOutboundJob
            });
        }
        settle_notification(&mut transaction, id, now).await?;
        transaction.commit().await.map_err(PersistenceError::Query)
    }

    /// Count jobs per state — the queue-depth metric source. States with zero rows are absent;
    /// labels are the schema's own state vocabulary, which carries no identifiers.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the query fails.
    pub async fn count_outbound_jobs_by_state(
        &self,
    ) -> Result<Vec<(String, i64)>, PersistenceError> {
        sqlx::query_as(
            "select state, count(*)::bigint
             from telegram.outbound_jobs
             group by state
             order by state",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(PersistenceError::Query)
    }
}

async fn settle_notification(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    now: i64,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "update telegram.notification_decisions decision
         set outcome = case job.state
                           when 'sent' then 'delivered'
                           when 'retry_wait' then 'retry_wait'
                           when 'failed_permanent' then 'failed_permanent'
                           when 'outcome_unknown' then 'outcome_unknown'
                           else decision.outcome
                       end,
             updated_at = to_timestamp($2)
         from telegram.outbound_jobs job
         where job.id = $1 and decision.outbound_job_id = job.id",
    )
    .bind(id)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}

async fn quarantine_expired_sends(
    transaction: &mut Transaction<'_, Postgres>,
    now: i64,
) -> Result<(), PersistenceError> {
    let quarantined: Vec<Uuid> = sqlx::query_scalar(
        "update telegram.outbound_jobs
         set state = 'outcome_unknown',
             lease_expires_at = null,
             last_error_class = 'process_exit_unknown',
             updated_at = to_timestamp($1)
         where state = 'sending' and kind = 'send_message'
           and lease_expires_at <= to_timestamp($1)
         returning id",
    )
    .bind(now)
    .fetch_all(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    if quarantined.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "update telegram.notification_decisions
         set outcome = 'outcome_unknown', updated_at = to_timestamp($2)
         where outbound_job_id = any($1)",
    )
    .bind(&quarantined)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}

/// Insert one outbound job inside a caller-owned atomic persistence operation.
pub(crate) async fn insert_outbound_job(
    transaction: &mut Transaction<'_, Postgres>,
    job: &NewOutboundJob,
    now: i64,
) -> Result<Uuid, PersistenceError> {
    if let (Some(operation_id), Some(revision)) = (job.operation_id, job.revision) {
        sqlx::query(
            "update telegram.outbound_jobs
             set state = 'superseded', updated_at = to_timestamp($3)
             where operation_id = $1 and chat_id = $2 and kind = 'edit_message_text'
               and state in ('planned', 'ready', 'retry_wait') and revision < $4",
        )
        .bind(operation_id)
        .bind(job.chat_id)
        .bind(now)
        .bind(revision)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    }

    let id = Uuid::now_v7();
    let payload = job.payload.canonical()?;
    let next_attempt_at = job.next_attempt_at.unwrap_or(now);
    sqlx::query(
        "insert into telegram.outbound_jobs
             (id, bot_id, chat_id, kind, payload, content_hash, operation_id,
              revision, correlation_id, next_attempt_at)
         values ($1, $2, $3, $4, $5::jsonb, $6, $7, $8, $9, to_timestamp($10))",
    )
    .bind(id)
    .bind(job.bot_id)
    .bind(job.chat_id)
    .bind(job.kind.as_str())
    .bind(payload)
    .bind(&job.content_hash)
    .bind(job.operation_id)
    .bind(job.revision)
    .bind(&job.correlation_id)
    .bind(next_attempt_at)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(id)
}

async fn settle_terminal(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    claim_attempt: i32,
    now: i64,
    state: OutboundJobState,
    error_class: Option<&String>,
) -> Result<sqlx::postgres::PgQueryResult, PersistenceError> {
    sqlx::query(
        "update telegram.outbound_jobs
         set state = $2,
             last_error_class = $3,
             lease_expires_at = null,
             updated_at = to_timestamp($4)
         where id = $1 and state = 'sending' and attempts = $5",
    )
    .bind(id)
    .bind(state.as_str())
    .bind(error_class)
    .bind(now)
    .bind(claim_attempt)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)
}
