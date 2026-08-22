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

use std::time::Duration;

/// How long a pooled connection may sit idle before it is closed rather than handed out.
///
/// Ten minutes. A pooled connection that outlives a database restart is the classic "it works on
/// the second try" failure, and this is the knob that bounds how long that window can be.
const IDLE_TIMEOUT: Duration = Duration::from_mins(10);

use secrecy::ExposeSecret as _;
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

    /// Apply [`SCHEMA`] to a database that does not have it yet.
    ///
    /// Idempotent, and safe to run while another process is still holding connections. One
    /// transaction does three things: it takes a `PostgreSQL` advisory lock, asks whether
    /// `telegram` exists, and applies the file only if it does not. The lock is transaction-scoped,
    /// so it is released by the commit and by a panic alike, and a second process that arrives
    /// during a restart waits for the first, then sees the schema and does nothing.
    ///
    /// `PostgreSQL` DDL is transactional, so a file that fails halfway leaves the database exactly
    /// as it was rather than half-applied. The presence check is therefore an honest question:
    /// either every object in the file is there or none of it is.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Schema`] if the lock cannot be taken, the catalogue cannot be read, or a
    /// statement in the file fails.
    pub async fn apply_schema(&self) -> Result<(), PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Schema)?;
        lock_and_apply(&mut transaction)
            .await
            .map_err(PersistenceError::Schema)?;
        transaction.commit().await.map_err(PersistenceError::Schema)
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
async fn lock_and_apply(connection: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(SCHEMA_LOCK)
        .execute(&mut *connection)
        .await?;

    // The schema the file creates. Under the lock, its absence means the file has never been
    // applied to this database.
    let present: Option<String> = sqlx::query_scalar("select to_regnamespace('telegram')::text")
        .fetch_one(&mut *connection)
        .await?;

    if present.is_none() {
        sqlx::Executor::execute(connection, SCHEMA).await?;
    }

    Ok(())
}
