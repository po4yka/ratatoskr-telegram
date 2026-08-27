//! Generalized dialogue and opaque interaction-token schema.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use telegram_persistence::test_support::TestDatabase;

#[tokio::test]
async fn fresh_schema_contains_generalized_dialogue_and_token_authority() {
    let test = TestDatabase::create().await.expect("database");

    let tables: Vec<String> = sqlx::query_scalar(
        "select table_name
         from information_schema.tables
         where table_schema = 'telegram'
           and table_name in ('dialog_states', 'interaction_tokens')
         order by table_name",
    )
    .fetch_all(test.pool())
    .await
    .expect("schema catalogue");
    assert_eq!(
        tables,
        ["dialog_states", "interaction_tokens"],
        "the current schema must expose both generalized interaction tables",
    );

    let dialogue_columns: Vec<String> = sqlx::query_scalar(
        "select column_name
         from information_schema.columns
         where table_schema = 'telegram' and table_name = 'dialog_states'",
    )
    .fetch_all(test.pool())
    .await
    .expect("dialogue columns");
    for required in [
        "id",
        "kind",
        "bot_id",
        "telegram_user_id",
        "chat_id",
        "expected_message_id",
        "step",
        "version",
        "lifecycle",
        "payload",
        "expires_at",
        "terminal_at",
    ] {
        assert!(dialogue_columns.iter().any(|column| column == required));
    }

    let token_columns: Vec<String> = sqlx::query_scalar(
        "select column_name
         from information_schema.columns
         where table_schema = 'telegram' and table_name = 'interaction_tokens'",
    )
    .fetch_all(test.pool())
    .await
    .expect("token columns");
    for required in [
        "token",
        "surface",
        "action",
        "bot_id",
        "telegram_user_id",
        "chat_id",
        "expected_message_id",
        "dialogue_id",
        "expected_dialogue_version",
        "operation_id",
        "payload",
        "expires_at",
        "consumed_at",
        "consumed_by_user",
    ] {
        assert!(token_columns.iter().any(|column| column == required));
    }

    let constrained: i64 = sqlx::query_scalar(
        "select count(*)
         from pg_constraint c
         join pg_class t on t.oid = c.conrelid
         join pg_namespace n on n.oid = t.relnamespace
         where n.nspname = 'telegram'
           and t.relname in ('dialog_states', 'interaction_tokens')
           and c.contype in ('c', 'f', 'p')",
    )
    .fetch_one(test.pool())
    .await
    .expect("interaction constraints");
    assert!(
        constrained >= 12,
        "scope, vocabulary, version, expiry, linkage, and consumption must be constrained",
    );
}

#[tokio::test]
async fn legacy_item_specific_interaction_tables_are_absent() {
    let test = TestDatabase::create().await.expect("database");
    let legacy: Vec<String> = sqlx::query_scalar(
        "select table_name from information_schema.tables
         where table_schema = 'telegram'
           and table_name in ('callback_flows', 'callback_tokens', 'interaction_intents')
         order by table_name",
    )
    .fetch_all(test.pool())
    .await
    .expect("schema catalogue");
    assert!(
        legacy.is_empty(),
        "superseded interaction tables remain: {legacy:?}"
    );
}
