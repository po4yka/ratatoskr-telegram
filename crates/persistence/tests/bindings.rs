//! The identity/chat binding tables: primary keys, the deferred internal binding, the closed
//! access vocabularies, and the enabled-by-default bootstrap. Each test runs against its own
//! disposable database.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use sqlx::Row;
use telegram_persistence::bindings::{AccessState, IdentityProfile};
use telegram_persistence::test_support::TestDatabase;

/// A connected pool over the disposable database, for raw assertions.
async fn database() -> TestDatabase {
    TestDatabase::create()
        .await
        .expect("a disposable database must be creatable")
}

/// `(data_type, is_nullable)` of one column of one `telegram` table.
async fn column_shape(
    db: &telegram_persistence::Database,
    table: &str,
    column: &str,
) -> (String, String) {
    let row = sqlx::query(
        "select data_type, is_nullable
         from information_schema.columns
         where table_schema = 'telegram'
           and table_name = $1
           and column_name = $2",
    )
    .bind(table)
    .bind(column)
    .fetch_one(db.pool())
    .await
    .expect("the catalog read");
    (row.get("data_type"), row.get("is_nullable"))
}

/// The binding tables exist on a fresh database with their primary keys, the nullable internal
/// binding, the closed vocabularies, and the enabled-by-default bootstrap.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one catalog walk over both tables reads better as one test than as two \
              half-tests sharing setup"
)]
fn identities_and_chats_exist_with_expected_shape() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;

        // The primary keys: one bigint identity column each.
        let identity_key = sqlx::query(
            "select a.attname as column_name
             from pg_index i
             join pg_class t on t.oid = i.indrelid
             join pg_attribute a on a.attrelid = t.oid and a.attnum = any(i.indkey)
             where t.relnamespace = 'telegram'::regnamespace
               and t.relname = 'identities'
               and i.indisprimary",
        )
        .fetch_all(test.pool())
        .await
        .expect("catalog read");
        let names: Vec<String> = identity_key
            .iter()
            .map(|row| row.get("column_name"))
            .collect();
        assert_eq!(names, ["telegram_user_id"]);

        let chat_key = sqlx::query(
            "select a.attname as column_name
             from pg_index i
             join pg_class t on t.oid = i.indrelid
             join pg_attribute a on a.attrelid = t.oid and a.attnum = any(i.indkey)
             where t.relnamespace = 'telegram'::regnamespace
               and t.relname = 'chats'
               and i.indisprimary",
        )
        .fetch_all(test.pool())
        .await
        .expect("catalog read");
        let names: Vec<String> = chat_key.iter().map(|row| row.get("column_name")).collect();
        assert_eq!(names, ["chat_id"]);

        // The Telegram-side keys are required; the Ratatoskr binding is deferred, so it is
        // explicitly nullable.
        assert_eq!(
            column_shape(&test.database, "identities", "telegram_user_id").await,
            ("bigint".to_owned(), "NO".to_owned()),
        );
        assert_eq!(
            column_shape(&test.database, "identities", "internal_user_id").await,
            ("uuid".to_owned(), "YES".to_owned()),
        );
        assert_eq!(
            column_shape(&test.database, "chats", "chat_id").await,
            ("bigint".to_owned(), "NO".to_owned()),
        );
        assert_eq!(
            column_shape(&test.database, "chats", "type").await,
            ("text".to_owned(), "NO".to_owned()),
        );

        // A bare identity row bootstraps enabled with its timestamps stamped.
        sqlx::query("insert into telegram.identities (telegram_user_id) values (700100200)")
            .execute(test.pool())
            .await
            .expect("the bare identity insert");
        let row = sqlx::query(
            "select access_state,
                    created_at is not null as created_stamped,
                    updated_at is not null as updated_stamped
             from telegram.identities where telegram_user_id = 700100200",
        )
        .fetch_one(test.pool())
        .await
        .expect("the identity row");
        assert_eq!(row.get::<&str, _>("access_state"), "enabled");
        assert!(row.get::<bool, _>("created_stamped"));
        assert!(row.get::<bool, _>("updated_stamped"));

        // A private chat row likewise boots enabled; the chat type is always explicit.
        sqlx::query("insert into telegram.chats (chat_id, type) values (900700601, 'private')")
            .execute(test.pool())
            .await
            .expect("the private chat insert");
        let row = sqlx::query("select access_state from telegram.chats where chat_id = 900700601")
            .fetch_one(test.pool())
            .await
            .expect("the chat row");
        assert_eq!(row.get::<&str, _>("access_state"), "enabled");

        // The access vocabulary is closed by CHECK constraints.
        let bogus_identity = sqlx::query(
            "insert into telegram.identities (telegram_user_id, access_state)
             values (1, 'suspended')",
        )
        .execute(test.pool())
        .await;
        assert!(
            bogus_identity.is_err(),
            "an unknown identity access_state must violate the check constraint"
        );
        let bogus_chat =
            sqlx::query("insert into telegram.chats (chat_id, type) values (2, 'group')")
                .execute(test.pool())
                .await;
        assert!(
            bogus_chat.is_err(),
            "a non-private chat must violate the check constraint"
        );
        let bogus_chat_access = sqlx::query(
            "insert into telegram.chats (chat_id, type, access_state)
             values (3, 'private', 'suspended')",
        )
        .execute(test.pool())
        .await;
        assert!(
            bogus_chat_access.is_err(),
            "an unknown chat access_state must violate the check constraint"
        );

        // The primary keys reject a second row for the same principal.
        let duplicate_identity =
            sqlx::query("insert into telegram.identities (telegram_user_id) values (700100200)")
                .execute(test.pool())
                .await;
        assert!(
            duplicate_identity.is_err(),
            "the same telegram user must not produce a second identity row"
        );
        let duplicate_chat =
            sqlx::query("insert into telegram.chats (chat_id, type) values (900700601, 'private')")
                .execute(test.pool())
                .await;
        assert!(
            duplicate_chat.is_err(),
            "the same telegram chat must not produce a second chat row"
        );

        test.cleanup().await.expect("cleanup");
    });
}

/// Ensure is insert-if-absent: an existing identity row comes back untouched — the snapshot is
/// not refreshed, a disabled state is not resurrected, and a bound Platform user id survives.
#[test]
fn ensuring_an_identity_again_never_touches_the_existing_row() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let created = db
            .ensure_identity(
                700_100_200,
                &IdentityProfile {
                    username: Some("owner".to_owned()),
                    ..IdentityProfile::default()
                },
            )
            .await
            .expect("the create");
        assert_eq!(created.telegram_user_id, 700_100_200);
        assert_eq!(created.access_state, AccessState::Enabled);
        assert_eq!(created.username.as_deref(), Some("owner"));
        assert_eq!(created.internal_user_id, None);

        // An operator-disabled identity that Platform has since bound: the re-ensure must
        // preserve all of it.
        let bound = uuid::Uuid::now_v7();
        sqlx::query(
            "update telegram.identities
             set access_state = 'disabled', username = 'changed', internal_user_id = $1
             where telegram_user_id = 700100200",
        )
        .bind(bound)
        .execute(db.pool())
        .await
        .expect("the manual mutation");

        let again = db
            .ensure_identity(700_100_200, &IdentityProfile::default())
            .await
            .expect("the re-ensure");
        assert_eq!(again.access_state, AccessState::Disabled);
        assert_eq!(again.username.as_deref(), Some("changed"));
        assert_eq!(again.internal_user_id, Some(bound));

        let found = db
            .find_identity(700_100_200)
            .await
            .expect("the find")
            .expect("the identity to be present");
        assert_eq!(found, again);

        let missing = db.find_identity(700_100_201).await.expect("the find");
        assert_eq!(missing, None);

        test.cleanup().await.expect("cleanup");
    });
}

/// Chats behave like identities: first ensure creates a private enabled row, later ensures
/// change nothing.
#[test]
fn ensuring_a_chat_creates_private_enabled_and_is_equally_stable() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;
        let db = &test.database;

        let unevaluated = db.find_chat(900_700_601).await.expect("the find");
        assert_eq!(unevaluated, None);

        let created = db.ensure_chat(900_700_601).await.expect("the create");
        assert_eq!(created.chat_id, 900_700_601);
        assert_eq!(created.access_state, AccessState::Enabled);
        let kind: String = sqlx::query("select type from telegram.chats where chat_id = 900700601")
            .fetch_one(db.pool())
            .await
            .expect("the chat row")
            .get("type");
        assert_eq!(kind, "private");

        sqlx::query(
            "update telegram.chats set access_state = 'disabled' where chat_id = 900700601",
        )
        .execute(db.pool())
        .await
        .expect("the manual mutation");

        let again = db.ensure_chat(900_700_601).await.expect("the re-ensure");
        assert_eq!(again.access_state, AccessState::Disabled);

        let found = db
            .find_chat(900_700_601)
            .await
            .expect("the find")
            .expect("the chat to be present");
        assert_eq!(found, again);

        test.cleanup().await.expect("cleanup");
    });
}
