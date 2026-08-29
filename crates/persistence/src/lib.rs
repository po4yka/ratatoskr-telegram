//! The `PostgreSQL` pool this service owns, the schema embedded in the binary, and the readiness
//! probe for both.
//!
//! Scope. This crate owns the `telegram` schema and nothing else. The service never reaches
//! Platform's or a domain service's tables, and the only thing that keeps that true over time is
//! that no code outside this crate is given a reason to hold a [`Database`] pointed anywhere else.
//!
//! The schema is ONE file, `schema.sql` at the repository root, not a numbered ledger. No database
//! holds data that has to survive a schema change, so an incremental history buys nothing and costs
//! a rule that an applied file can never be edited. A schema change edits `schema.sql` in place.

#[cfg(feature = "test-support")]
pub mod test_support;

pub mod bindings;
pub mod capture_projection;
pub mod dialogues;
pub mod inbox;
pub mod interaction_cleanup;
pub mod interaction_tokens;
pub mod message_bindings;
pub mod notification_delivery;
pub mod notification_preferences;
pub mod outbound_jobs;
pub mod projection_accept;
pub mod updates;

pub use bindings::{AccessState, ChatRecord, IdentityProfile, IdentityRecord};
pub use message_bindings::MessageBindingRecord;
pub use notification_preferences::{
    NotificationPreferenceUpdate, NotificationPreferences, QuietPolicy,
};
pub use outbound_jobs::{
    DeliveryOutcome, NewOutboundJob, OutboundJobKind, OutboundJobState, QueuedOutboundJob,
};
pub use projection_accept::{AcceptOutcome, AcceptedEvent};
pub use updates::{AdmittedUpdate, PendingUpdate, RecordOutcome, UpdateState};

use std::time::Duration;

/// How long a pooled connection may sit idle before it is closed rather than handed out.
///
/// Ten minutes. A pooled connection that outlives a database restart is the classic "it works on
/// the second try" failure, and this is the knob that bounds how long that window can be.
const IDLE_TIMEOUT: Duration = Duration::from_mins(10);

use secrecy::ExposeSecret as _;
use sha2::{Digest as _, Sha256};
use sqlx::postgres::{PgPool, PgPoolOptions};
use telegram_core::{Subsystem, TelegramError};

/// The schema, embedded at compile time.
///
/// Embedded rather than read from disk so a deployed binary cannot be paired with a different
/// schema than the one it was built against. `include_str!` makes the file a build input, so
/// editing it rebuilds this crate and every artifact that links it — which is the whole of the
/// staleness protection a directory of migration files would otherwise have to provide. The path is
/// relative to this source file.
const SCHEMA: &str = include_str!("../../../schema.sql");

/// The advisory-lock key `apply_schema` holds while it decides and applies.
///
/// One arbitrary but fixed 64-bit value; `PostgreSQL` advisory locks are a namespace of integers
/// with no meaning of their own, and nothing else in this system takes one. Kept because a restart
/// that overlaps the previous process's grace window is two processes, for a few seconds, and both
/// call this method. The value is distinct from every other service's key by construction: it is
/// the ASCII of `rataskr` plus this service's own suffix.
const SCHEMA_LOCK: i64 = 0x7261_7461_736b_7202;

/// A failure in the pool, the schema, or a query.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PersistenceError {
    /// The pool could not be created, or a connection could not be acquired.
    #[error("the database connection could not be established")]
    Connect(#[source] sqlx::Error),

    /// The schema could not be applied.
    #[error("the database schema could not be applied")]
    Schema(#[source] sqlx::Error),

    /// A query failed.
    #[error("a database query failed")]
    Query(#[source] sqlx::Error),

    /// Accepted-capture projection inputs disagree about their bot, chat, operation, or shape.
    #[error("the accepted capture projection is internally inconsistent")]
    InvalidCaptureProjection,

    /// A settlement named an update that was never admitted. A state transition for a row that
    /// does not exist is a bug, and silently succeeding would hide it.
    #[error("the update was never admitted")]
    UnknownUpdate,

    /// A binding mutation named an `(operation_id, chat_id)` pair with no binding row. Same rule
    /// as [`PersistenceError::UnknownUpdate`]: a transition for a row that does not exist is a
    /// bug, and reporting it as "already terminal" or "already unbound" would corrupt the
    /// caller's decision.
    #[error("the message binding was never created")]
    UnknownBinding,

    /// A settlement named an outbound job that was never enqueued. Same rule as
    /// [`PersistenceError::UnknownUpdate`]: settling a job nobody can find would silently drop
    /// work the queue promised to deliver.
    #[error("the outbound job was never enqueued")]
    UnknownOutboundJob,

    /// An optimistic notification-preference update named an old version.
    #[error("the notification preference version is stale")]
    StalePreference,

    /// A notification policy was invalid before it reached the database.
    #[error("the notification preference is invalid")]
    InvalidPreference,

    /// The dispatcher reached a database before the webhook role applied the current schema.
    #[error("the telegram schema is absent or incomplete")]
    SchemaAbsent,

    /// The database contains a Telegram schema created from a different or unverifiable
    /// definition. Development databases are recreated instead of migrated in place.
    #[error("the telegram schema does not match the running binary")]
    SchemaMismatch,
}

impl From<PersistenceError> for TelegramError {
    /// Every persistence failure is internal. There is no variant a client learns about: a
    /// connection string, a constraint name and a query are all internal detail.
    fn from(error: PersistenceError) -> Self {
        Self::Internal {
            subsystem: Subsystem::Persistence,
            source: Box::new(error),
        }
    }
}

/// The pool, and the only handle through which this service reaches `PostgreSQL`.
#[derive(Debug, Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Create the pool and verify it can serve one connection.
    ///
    /// The verification is not ceremony: it is still possible to hold a pool whose credentials are
    /// wrong, and finding that out on the first request rather than at startup is how a deployment
    /// reports itself healthy and then fails every call.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Connect`] if the URL is unusable or the server refuses the connection
    /// within the configured acquire timeout.
    pub async fn connect(config: &telegram_core::DatabaseConfig) -> Result<Self, PersistenceError> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(Duration::from_secs(config.acquire_timeout_seconds))
            .idle_timeout(IDLE_TIMEOUT)
            .test_before_acquire(true)
            .connect(config.url.expose_secret())
            .await
            .map_err(PersistenceError::Connect)?;

        Ok(Self { pool })
    }

    /// Apply [`SCHEMA`] to a fresh database or verify an existing exact match.
    ///
    /// Idempotent, and safe to run while another process is still holding connections. One
    /// transaction takes a `PostgreSQL` advisory lock and either applies the file plus its exact
    /// fingerprint to an empty database, or proves that an existing namespace carries the same
    /// fingerprint. The lock is transaction-scoped, so it is released by the commit and by a panic
    /// alike, and a second process that arrives during a restart waits for the first.
    ///
    /// `PostgreSQL` DDL is transactional, so a file or fingerprint insert that fails halfway leaves
    /// the database exactly as it was rather than half-applied.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Schema`] if the lock cannot be taken, the catalogue cannot be read, or a
    /// statement in the file fails; [`PersistenceError::SchemaMismatch`] if an existing namespace
    /// does not carry evidence for this binary's embedded definition.
    pub async fn apply_schema(&self) -> Result<(), PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Schema)?;
        lock_and_apply(&mut transaction).await?;
        transaction.commit().await.map_err(PersistenceError::Schema)
    }

    /// Verify the current schema exists without applying or mutating it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::SchemaAbsent`] when the owned namespace is absent,
    /// [`PersistenceError::SchemaMismatch`] when its definition evidence is absent or different,
    /// or [`PersistenceError::Query`] when the catalog cannot be read.
    pub async fn verify_schema(&self) -> Result<(), PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Query)?;
        lock_schema(&mut transaction)
            .await
            .map_err(PersistenceError::Query)?;
        let present = namespace_present(&mut transaction)
            .await
            .map_err(PersistenceError::Query)?;
        if !present {
            return Err(PersistenceError::SchemaAbsent);
        }
        let stored = read_schema_fingerprint(&mut transaction)
            .await
            .map_err(PersistenceError::Query)?;
        ensure_schema_matches(stored.as_deref())?;
        transaction.commit().await.map_err(PersistenceError::Query)
    }

    /// Answer whether the database is usable right now.
    ///
    /// Deliberately a round trip and not a pool-state inspection: a pool with idle connections to a
    /// server that is refusing queries looks healthy from the inside.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if the round trip fails or times out.
    pub async fn ping(&self) -> Result<(), PersistenceError> {
        sqlx::query("select 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(PersistenceError::Query)
    }

    /// The pool, for the crates that own queries against this schema.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Close the pool and wait for checked-out connections to be returned.
    ///
    /// Called from the shutdown sequence after the listener stops accepting, so an in-flight
    /// request keeps its connection through the grace window.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// The body of [`Database::apply_schema`], on one connection so the lock and the apply share a
/// session.
///
/// A free function taking `&mut PgConnection` by its named type: `PublicRoutes::build` is an async
/// trait method, so a caller has to prove the future is `Send`, and that proof needs the executor's
/// lifetime pinned rather than inferred at the call site (rust-lang/rust#100013, seen as
/// "implementation of `Executor` is not general enough").
///
/// The file goes through `Executor::execute` and NOT `sqlx::raw_sql`, which trips the same bound.
/// Both send the string over the simple query protocol, which runs every statement in it; `execute`
/// folds the per-statement results into one.
async fn lock_and_apply(connection: &mut sqlx::PgConnection) -> Result<(), PersistenceError> {
    lock_schema(connection)
        .await
        .map_err(PersistenceError::Schema)?;

    if namespace_present(connection)
        .await
        .map_err(PersistenceError::Schema)?
    {
        let stored = read_schema_fingerprint(connection)
            .await
            .map_err(PersistenceError::Schema)?;
        return ensure_schema_matches(stored.as_deref());
    }

    sqlx::Executor::execute(&mut *connection, SCHEMA)
        .await
        .map_err(PersistenceError::Schema)?;
    sqlx::query("insert into telegram.schema_fingerprint (singleton, sha256) values (true, $1)")
        .bind(schema_fingerprint().as_slice())
        .execute(connection)
        .await
        .map_err(PersistenceError::Schema)?;
    Ok(())
}

async fn lock_schema(connection: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(SCHEMA_LOCK)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn namespace_present(connection: &mut sqlx::PgConnection) -> Result<bool, sqlx::Error> {
    let present: Option<String> = sqlx::query_scalar("select to_regnamespace('telegram')::text")
        .fetch_one(&mut *connection)
        .await?;
    Ok(present.is_some())
}

async fn read_schema_fingerprint(
    connection: &mut sqlx::PgConnection,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let authority_present: bool =
        sqlx::query_scalar("select to_regclass('telegram.schema_fingerprint') is not null")
            .fetch_one(&mut *connection)
            .await?;
    if !authority_present {
        return Ok(None);
    }

    sqlx::query_scalar("select sha256 from telegram.schema_fingerprint where singleton = true")
        .fetch_optional(connection)
        .await
}

fn ensure_schema_matches(stored: Option<&[u8]>) -> Result<(), PersistenceError> {
    if stored == Some(schema_fingerprint().as_slice()) {
        Ok(())
    } else {
        Err(PersistenceError::SchemaMismatch)
    }
}

fn schema_fingerprint() -> [u8; 32] {
    Sha256::digest(SCHEMA.as_bytes()).into()
}
