//! The projection accept step: one transactional guard sequence per operation event.
//!
//! Guard precedence is the spec's, and it lives in ONE transaction so a redelivered or stale
//! event can never half-apply: inbox deduplication first; then binding lookup; terminal flag;
//! staleness against the accept watermark (`last_event_at`); revision assignment; the supersede
//! sweep and job insert; and the watermark advance. Every outcome either commits all of its
//! writes or none of them.
//!
//! Two asymmetries are deliberate:
//!
//! - **Unbound leaves zero writes** — not even an inbox row. Recording the dedup row for an
//!   event whose binding does not exist yet would poison that `event_id` forever: once the bind
//!   lands mid-flight, the redelivered event that could have rendered it is already "consumed".
//!   Events carry snapshots (state-carried), so nothing is lost by leaving no trace.
//! - **Duplicate/PostTerminal/Stale keep their inbox row** — the evidence that this envelope was
//!   seen and judged must stick, so a redelivery of the same id short-circuits instead of
//!   re-entering the guards.

use sqlx::types::Uuid;

use crate::message_bindings::{
    BINDING_COLUMNS, BindingRow, MessageBindingRecord, binding_from_row,
};
use crate::{Database, PersistenceError};

/// The event fields the accept step needs, gathered so the method call stays small. `body` and
/// `content_hash` are computed by the caller (render + sha256) because rendering is dispatcher
/// policy, not persistence's.
#[derive(Debug, Clone, Copy)]
pub struct AcceptedEvent<'a> {
    /// The Platform operation this snapshot belongs to.
    pub operation_id: Uuid,
    /// The envelope's globally unique occurrence id — the inbox dedup key.
    pub event_id: Uuid,
    /// When the producer observed the fact, whole seconds since the Unix epoch.
    pub occurred_at_secs: i64,
    /// Whether the snapshot's status is terminal (consumer-side closed enum).
    pub terminal: bool,
    /// The rendered Telegram HTML body.
    pub body: &'a str,
    /// sha256 hex of [`Self::body`].
    pub content_hash: &'a str,
    /// The contracts `EntityRef` correlation string, carried onto the job for tracing only.
    pub correlation_id: &'a str,
}

/// How one accepted event ended. [`AcceptOutcome::Recorded`] carries the assigned projection
/// revision; every other outcome names the guard that dropped it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptOutcome {
    /// All guards passed: the job was enqueued and the watermark advanced.
    Recorded {
        /// The revision assigned to this render (`last_rendered_revision + 1`).
        revision: i64,
    },
    /// This exact envelope was already consumed.
    Duplicate,
    /// The binding is already terminal; the event arrived after the end.
    PostTerminal,
    /// The event is older than the newest accepted one.
    Stale,
    /// No binding exists for the operation; nothing at all was written.
    Unbound,
}

impl Database {
    /// Run the whole guard sequence for one operation event inside ONE transaction.
    ///
    /// The caller renders and hashes beforehand; this layer owns ordering, atomicity, and the
    /// vocabulary of outcomes. On [`AcceptOutcome::Recorded`] the job's earliest attempt honors
    /// the render interval anchored at the last DELIVERED render (design D4): terminals skip the
    /// delay, everything else waits out `max(now, last_rendered_at + interval)`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if any statement fails; the transaction rolls back and no
    /// guard outcome is reported.
    pub async fn accept_operation_event(
        &self,
        event: AcceptedEvent<'_>,
        now: i64,
        render_interval_secs: i64,
    ) -> Result<AcceptOutcome, PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Query)?;

        // 1. The insert IS the dedup decision; a redelivered envelope stops here.
        if !insert_inbox(&mut transaction, event.event_id).await? {
            return Ok(AcceptOutcome::Duplicate);
        }

        // 2. A binding must exist. Its absence rolls the inbox row back too — by design, so a
        // bind landing later can still consume this envelope's redelivery.
        let Some(binding) = lock_binding(&mut transaction, event.operation_id).await? else {
            return Ok(AcceptOutcome::Unbound);
        };

        // 3-4. Terminal and staleness guards. Both keep the inbox evidence committed: the
        // envelope was seen and judged, and a redelivery must short-circuit.
        if binding.terminal {
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(AcceptOutcome::PostTerminal);
        }
        if binding
            .last_event_at
            .is_some_and(|watermark| event.occurred_at_secs < watermark)
        {
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(AcceptOutcome::Stale);
        }

        // 5-7. Revision assignment, then the guarded terminal flip; losing that race is the
        // same post-terminal outcome as arriving after a settled flag.
        let revision = next_revision(&mut transaction, &binding).await?;
        if event.terminal && !flip_terminal(&mut transaction, &binding, now).await? {
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(AcceptOutcome::PostTerminal);
        }

        // 8-9. Sweep older waiting edits, insert this render's job, advance the watermark —
        // all inside the same transaction as every guard above.
        supersede_older_edits(&mut transaction, &binding, revision, now).await?;
        insert_edit_job(
            &mut transaction,
            &binding,
            &event,
            revision,
            now,
            render_interval_secs,
        )
        .await?;
        advance_watermark(&mut transaction, &binding, event.occurred_at_secs).await?;

        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(AcceptOutcome::Recorded { revision })
    }
}

/// Step 1: the inbox insert-or-ignore. `true` when this call is the first arrival.
async fn insert_inbox(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: Uuid,
) -> Result<bool, PersistenceError> {
    let inserted: Option<bool> = sqlx::query_scalar(
        "insert into telegram.inbox (event_id)
         values ($1)
         on conflict (event_id) do nothing
         returning true",
    )
    .bind(event_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(inserted.is_some())
}

/// Step 2: read and lock the binding for one operation, so concurrent accepts of one binding
/// serialize instead of racing through revision assignment and the watermark advance.
async fn lock_binding(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: Uuid,
) -> Result<Option<MessageBindingRecord>, PersistenceError> {
    let row: Option<BindingRow> = sqlx::query_as(&format!(
        "select {BINDING_COLUMNS}
         from telegram.message_bindings
         where operation_id = $1
         order by chat_id
         limit 1
         for update"
    ))
    .bind(operation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(row.map(binding_from_row))
}

/// Step 5: the assigned revision. Accept-time revisions (design D3) may run ahead of delivered
/// renders, so the watermark is the highest revision ever enqueued for this binding — superseded
/// rows included, they still hold theirs — floored at the delivered watermark so history pruning
/// can never make a new assignment regress.
async fn next_revision(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    binding: &MessageBindingRecord,
) -> Result<i64, PersistenceError> {
    let assigned_floor: i64 = sqlx::query_scalar(
        "select coalesce(max(revision), 0)::bigint
         from telegram.outbound_jobs
         where operation_id = $1 and chat_id = $2",
    )
    .bind(binding.operation_id)
    .bind(binding.chat_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(assigned_floor.max(binding.last_rendered_revision) + 1)
}

/// Step 7: flip the terminal flag under its `not terminal` guard. `false` means another event
/// won the race — the caller reports post-terminal.
async fn flip_terminal(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    binding: &MessageBindingRecord,
    now: i64,
) -> Result<bool, PersistenceError> {
    let flipped: Option<bool> = sqlx::query_scalar(
        "update telegram.message_bindings
         set terminal = true,
             updated_at = to_timestamp($3)
         where operation_id = $1 and chat_id = $2 and not terminal
         returning true",
    )
    .bind(binding.operation_id)
    .bind(binding.chat_id)
    .bind(now)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(flipped.is_some())
}

/// Step 8a: withdraw every older still-waiting edit of this binding — the enqueue-time sweep,
/// replicated here so it lands atomically with the newer render's arrival.
async fn supersede_older_edits(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    binding: &MessageBindingRecord,
    revision: i64,
    now: i64,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "update telegram.outbound_jobs
         set state = 'superseded',
             updated_at = to_timestamp($3)
         where operation_id = $1
           and chat_id = $2
           and kind = 'edit_message_text'
           and state in ('planned', 'ready', 'retry_wait')
           and revision < $4",
    )
    .bind(binding.operation_id)
    .bind(binding.chat_id)
    .bind(now)
    .bind(revision)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}

/// Step 8b: insert the edit job. Terminals skip the interval delay; everything else anchors at
/// the last DELIVERED render (design D4), never at the accept instant.
async fn insert_edit_job(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    binding: &MessageBindingRecord,
    event: &AcceptedEvent<'_>,
    revision: i64,
    now: i64,
    render_interval_secs: i64,
) -> Result<(), PersistenceError> {
    let throttle_floor = binding
        .last_rendered_at
        .unwrap_or(0)
        .saturating_add(render_interval_secs.max(0));
    sqlx::query(
        "insert into telegram.outbound_jobs
             (id, bot_id, chat_id, kind, body, content_hash, operation_id, revision,
              correlation_id, next_attempt_at)
         values ($1, $2, $3, 'edit_message_text', $4, $5, $6, $7, $8,
                 case when $9 then to_timestamp($10)
                      else to_timestamp(greatest($10, $11)) end)",
    )
    .bind(Uuid::now_v7())
    .bind(binding.bot_id)
    .bind(binding.chat_id)
    .bind(event.body)
    .bind(event.content_hash)
    .bind(event.operation_id)
    .bind(revision)
    .bind(event.correlation_id)
    .bind(event.terminal)
    .bind(now)
    .bind(throttle_floor)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}

/// Step 9: the accept watermark moves only here, transactionally with the job insert.
async fn advance_watermark(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    binding: &MessageBindingRecord,
    occurred_at_secs: i64,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "update telegram.message_bindings
         set last_event_at = to_timestamp($3),
             updated_at = to_timestamp($3)
         where operation_id = $1 and chat_id = $2",
    )
    .bind(binding.operation_id)
    .bind(binding.chat_id)
    .bind(occurred_at_secs)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}
