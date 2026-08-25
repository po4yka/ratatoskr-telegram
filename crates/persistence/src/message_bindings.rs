//! Operation message bindings: the one-live-binding-per-(operation, chat) ledger the dispatcher
//! edits progress through.
//!
//! A binding anchors a Platform operation to one Telegram chat message so later events edit that
//! message instead of sending new ones. Three invariants live here rather than in callers:
//! insert-if-absent creation (`ensure_operation_binding`, mirroring the identity/chat `ensure_*`
//! discipline), acknowledgment-gated message ids (`record_send_acknowledged` is the ONLY writer
//! of `message_id`, and it runs after a Bot API ack, never from an attempt still in flight), and
//! monotonic revisions (`advance_render` refuses any revision not strictly newer than the last
//! rendered one, so out-of-order or replayed events cannot regress a render).
//!
//! Every timestamp is caller-supplied (whole seconds since the Unix epoch) and converted at the
//! SQL boundary with `to_timestamp`, so throttling and render arithmetic stay deterministic under
//! an injected clock; no query reads the database clock.

use sqlx::types::Uuid;

use crate::{Database, PersistenceError};

/// One live binding of a Platform operation to one Telegram chat message, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageBindingRecord {
    /// App-minted `UUIDv7` primary key.
    pub id: Uuid,
    /// The bot the bound message belongs to.
    pub bot_id: i64,
    /// The Platform operation this binding renders. An unenforced reference across schemas; the
    /// application owns the invariant.
    pub operation_id: Uuid,
    /// The chat the bound message lives in.
    pub chat_id: i64,
    /// The Telegram message id, present only between a send acknowledgment and an unbind.
    pub message_id: Option<i64>,
    /// The newest projection revision rendered into the bound message; never decreases.
    pub last_rendered_revision: i64,
    /// When [`Self::last_rendered_revision`] was rendered, whole seconds since the Unix epoch.
    pub last_rendered_at: Option<i64>,
    /// Whether a terminal projection has been accepted for this binding.
    pub terminal: bool,
}

/// The column tuple a binding read maps from: identity keys, the acknowledgment-gated message
/// id, the monotonic render state, and the terminal flag. The timestamp crosses as
/// `extract(epoch ...)` — the crate carries no date-time dependency by design.
type BindingRow = (Uuid, i64, Uuid, i64, Option<i64>, i64, Option<i64>, bool);

/// The projection columns of one binding, keyed by its (operation, chat) pair. The timestamp is
/// read out as epoch seconds to match the caller-supplied form writes take.
const BINDING_COLUMNS: &str = "id, bot_id, operation_id, chat_id, message_id, \
     last_rendered_revision, extract(epoch from last_rendered_at)::bigint, terminal";

impl Database {
    /// Return the binding for `(operation_id, chat_id)`, creating it empty when absent.
    ///
    /// Insert-if-absent like the identity/chat ensures: whatever an existing row says stays
    /// authoritative, so a re-ensure after a crash never resets a revision, clears a terminal
    /// flag, or disturbs a bound message id. The id is minted here (`UUIDv7`) because the schema
    /// deliberately gives id columns no DEFAULT — a missing id must be an error at the writer,
    /// not a silently wrong version at the database.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if either statement fails.
    pub async fn ensure_operation_binding(
        &self,
        bot_id: i64,
        operation_id: Uuid,
        chat_id: i64,
    ) -> Result<MessageBindingRecord, PersistenceError> {
        // Insert-if-absent, then read back: the read is the authoritative answer whether this
        // call created the row or lost the race against one that did.
        sqlx::query(
            "insert into telegram.message_bindings (id, bot_id, operation_id, chat_id)
             values ($1, $2, $3, $4)
             on conflict (operation_id, chat_id) do nothing",
        )
        .bind(Uuid::now_v7())
        .bind(bot_id)
        .bind(operation_id)
        .bind(chat_id)
        .execute(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        let row: BindingRow = sqlx::query_as(&format!(
            "select {BINDING_COLUMNS}
             from telegram.message_bindings
             where operation_id = $1 and chat_id = $2"
        ))
        .bind(operation_id)
        .bind(chat_id)
        .fetch_one(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        Ok(binding_from_row(row))
    }

    /// Record a Bot API send acknowledgment, upserting the returned message id.
    ///
    /// This is the sender establishing the binding after success (design D7): the first ack
    /// creates the row with its message id, every later ack for the same pair updates the id in
    /// place. One statement does both, so two racing acks converge on one row instead of
    /// duplicating it. Called only after the Bot API acknowledged — provider message ids are
    /// never recorded from unacknowledged attempts.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails.
    pub async fn record_send_acknowledged(
        &self,
        bot_id: i64,
        operation_id: Uuid,
        chat_id: i64,
        message_id: i64,
        now: i64,
    ) -> Result<(), PersistenceError> {
        sqlx::query(
            "insert into telegram.message_bindings
                 (id, bot_id, operation_id, chat_id, message_id, updated_at)
             values ($1, $2, $3, $4, $5, to_timestamp($6))
             on conflict (operation_id, chat_id) do update
             set message_id = excluded.message_id,
                 updated_at = excluded.updated_at",
        )
        .bind(Uuid::now_v7())
        .bind(bot_id)
        .bind(operation_id)
        .bind(chat_id)
        .bind(message_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;
        Ok(())
    }

    /// Advance the binding's render state to `revision`, stamping `rendered_at`.
    ///
    /// Conditional on `last_rendered_revision < revision`: the write applies only when the named
    /// revision is strictly newer than what the binding already rendered, and the guard lives in
    /// the UPDATE itself so two racing consumers cannot both win. Returns `false` for a stale
    /// (already-rendered or older) revision without touching the row — the caller treats that as
    /// "drop, nothing changed". A binding that does not exist also reports `false`: the consumer
    /// only advances bindings it has just ensured or found, so absence degrades to the same
    /// harmless drop rather than a distinct failure class.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the statement fails.
    pub async fn advance_render(
        &self,
        operation_id: Uuid,
        chat_id: i64,
        revision: i64,
        rendered_at: i64,
    ) -> Result<bool, PersistenceError> {
        let applied: Option<bool> = sqlx::query_scalar(
            "update telegram.message_bindings
             set last_rendered_revision = $3,
                 last_rendered_at = to_timestamp($4),
                 updated_at = to_timestamp($4)
             where operation_id = $1 and chat_id = $2 and last_rendered_revision < $3
             returning true",
        )
        .bind(operation_id)
        .bind(chat_id)
        .bind(revision)
        .bind(rendered_at)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        Ok(applied.is_some())
    }

    /// The binding for `(operation_id, chat_id)`, if one exists.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the query fails.
    pub async fn find_binding(
        &self,
        operation_id: Uuid,
        chat_id: i64,
    ) -> Result<Option<MessageBindingRecord>, PersistenceError> {
        let row: Option<BindingRow> = sqlx::query_as(&format!(
            "select {BINDING_COLUMNS}
             from telegram.message_bindings
             where operation_id = $1 and chat_id = $2"
        ))
        .bind(operation_id)
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        Ok(row.map(binding_from_row))
    }

    /// Clear the binding's message id, keeping the row and its revision history.
    ///
    /// Used after a permanent edit failure (message deleted, cannot be edited): the next revision
    /// sends a fresh message and rebinds instead of all rendering for the operation dying.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::UnknownBinding`] when no row carries the pair;
    /// [`PersistenceError::Query`] if the statement fails otherwise.
    pub async fn unbind_message(
        &self,
        operation_id: Uuid,
        chat_id: i64,
        now: i64,
    ) -> Result<(), PersistenceError> {
        let result = sqlx::query(
            "update telegram.message_bindings
             set message_id = null,
                 updated_at = to_timestamp($3)
             where operation_id = $1 and chat_id = $2",
        )
        .bind(operation_id)
        .bind(chat_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        if result.rows_affected() == 0 {
            return Err(PersistenceError::UnknownBinding);
        }
        Ok(())
    }

    /// Set the terminal flag, once.
    ///
    /// The flag flip is guarded by `where not terminal`, so exactly one caller's write lands:
    /// `true` means this call flipped the flag, `false` means the binding was already terminal.
    /// Downstream drops second terminals and post-terminal events on exactly this distinction,
    /// which is why absence of the row is an ERROR rather than a folded-in `false` — dropping a
    /// live operation's events because its binding was misnamed would be a silent data loss.
    /// Bindings are never deleted once created, so the follow-up read cannot disagree with the
    /// failed flip about whether the row exists.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::UnknownBinding`] when no row carries the pair;
    /// [`PersistenceError::Query`] if a statement fails otherwise.
    pub async fn mark_terminal(
        &self,
        operation_id: Uuid,
        chat_id: i64,
        now: i64,
    ) -> Result<bool, PersistenceError> {
        let flipped: Option<bool> = sqlx::query_scalar(
            "update telegram.message_bindings
             set terminal = true,
                 updated_at = to_timestamp($3)
             where operation_id = $1 and chat_id = $2 and not terminal
             returning true",
        )
        .bind(operation_id)
        .bind(chat_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        if flipped.is_some() {
            return Ok(true);
        }

        // No flip: either the binding was already terminal or it does not exist. The caller
        // needs to know which.
        let exists: Option<bool> = sqlx::query_scalar(
            "select true
             from telegram.message_bindings
             where operation_id = $1 and chat_id = $2",
        )
        .bind(operation_id)
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;

        match exists {
            Some(true) => Ok(false),
            _ => Err(PersistenceError::UnknownBinding),
        }
    }
}

/// Assemble a [`MessageBindingRecord`] from its column tuple.
fn binding_from_row(row: BindingRow) -> MessageBindingRecord {
    MessageBindingRecord {
        id: row.0,
        bot_id: row.1,
        operation_id: row.2,
        chat_id: row.3,
        message_id: row.4,
        last_rendered_revision: row.5,
        last_rendered_at: row.6,
        terminal: row.7,
    }
}
