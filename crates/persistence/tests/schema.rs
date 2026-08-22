//! The embedded `telegram` schema: applied fresh, idempotent, all-or-nothing.
//!
//! It replaces nothing and precedes every table: what is worth checking before any table exists is
//! the one branch everything later depends on. [`Database::apply_schema`] knows a database is up to
//! date by asking whether `telegram` exists, under an advisory lock. Get that wrong and the second
//! start of a process fails with `schema "telegram" already exists` — which is every restart, on
//! the one host there is.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use secrecy::SecretString;
use sqlx::Executor as _;
use sqlx::postgres::PgPoolOptions;
use telegram_core::DatabaseConfig;
use telegram_persistence::Database;

/// Where the disposable database is created.
///
/// The same variable and the same default the rest of the suite uses, so `docker compose up -d`
/// followed by `cargo test` needs no further setup.
#[expect(
    clippy::disallowed_methods,
    reason = "the workspace bans direct environment reads so that configuration has exactly one \
              loader. This is a test binary choosing where it may create and drop a database."
)]
fn admin_url() -> String {
    std::env::var("TELEGRAM_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://telegram:telegram@127.0.0.1:5432/telegram".to_owned())
}

/// A pool configuration for `url`, bounded the way a test is.
fn config(url: &str) -> DatabaseConfig {
    DatabaseConfig {
        url: SecretString::from(url.to_owned()),
        max_connections: 2,
        acquire_timeout_seconds: 5,
    }
}

/// Creates a uniquely named empty database and hands back `(url, name)`; the caller drops it.
///
/// The same three clauses `test_support` uses: template0 so no cluster-local objects come along,
/// ICU with the fixed locale CI and `compose.yaml` create their clusters with, UTF8 encoding.
async fn create_database() -> (String, String) {
    let admin = admin_url();
    let name = format!("telegram_schema_{}", uuid::Uuid::now_v7().simple());

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin)
        .await
        .expect("the test database server must be reachable");
    pool.execute(
        format!(
            r#"create database "{name}" template template0
               locale_provider icu icu_locale 'und-x-icu' encoding 'UTF8'"#
        )
        .as_str(),
    )
    .await
    .expect("a fresh database must be creatable");
    pool.close().await;

    let url = match admin.rsplit_once('/') {
        Some((prefix, _)) => format!("{prefix}/{name}"),
        None => format!("{admin}/{name}"),
    };
    (url, name)
}

/// Drops the named database, even when the test that created it failed halfway.
async fn drop_database(name: &str) {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect("the test database server must stay reachable");
    pool.execute(format!(r#"drop database if exists "{name}" with (force)"#).as_str())
        .await
        .expect("cleanup must succeed");
    pool.close().await;
}

/// A fresh database receives the whole file in one application, and applying it again is a no-op:
/// the branch a restart takes, every time.
#[tokio::test]
async fn the_schema_applies_once_and_tolerates_being_applied_again() {
    let (url, name) = create_database().await;

    let database = Database::connect(&config(&url))
        .await
        .expect("a fresh database must be connectable");

    database
        .apply_schema()
        .await
        .expect("the first application succeeds");

    let present: Option<String> = sqlx::query_scalar("select to_regnamespace('telegram')::text")
        .fetch_one(database.pool())
        .await
        .expect("the catalogue is readable");
    assert_eq!(present.as_deref(), Some("telegram"));

    // The second application is what a concurrent or restarted process does. It must succeed and
    // change nothing: not re-run the file, not leave a second copy of anything.
    database
        .apply_schema()
        .await
        .expect("a second application is idempotent");

    let schemas: i64 = sqlx::query_scalar(
        "select count(*) from information_schema.schemata where schema_name = 'telegram'",
    )
    .fetch_one(database.pool())
    .await
    .expect("the catalogue is readable");
    assert_eq!(schemas, 1, "exactly one telegram schema exists");

    // And the round trip the readiness probe makes answers on the same pool.
    database.ping().await.expect("a live pool answers ping");

    database.close().await;
    drop_database(&name).await;
}

/// A database whose URL is wrong fails at CONNECT, with an error that carries no credential.
#[tokio::test]
async fn an_unreachable_database_fails_at_connect_without_leaking_the_url() {
    let error = Database::connect(&config("postgres://nobody:nope@127.0.0.1:5/nowhere"))
        .await
        .expect_err("nothing listens there");
    let rendered = format!("{error}");
    assert!(
        !rendered.contains("nope") && !rendered.contains("nowhere"),
        "the connect error echoed configuration: {rendered}",
    );
}
