//! Bounded cleanup of stale interaction authority.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use telegram_persistence::dialogues::{
    DialogueLifecycle, DialogueScope, GitHubRepositoryDialogue, NewGitHubDialogue,
};
use telegram_persistence::interaction_tokens::{
    LibraryReadScope, NewLibraryReadIntent, NewOperationIntent, OperationIntentPayload,
    TokenPresentation, TokenScope, TokenSurface,
};
use telegram_persistence::test_support::TestDatabase;

const T0: i64 = 1_800_000_000;
const BOT: i64 = 42;
const OWNER: i64 = 900_700_601;

#[tokio::test]
async fn cleanup_removes_expired_library_read_authority() {
    let test = TestDatabase::create().await.expect("database");
    let token = test
        .database
        .issue_library_read_intent(
            NewLibraryReadIntent {
                scope: LibraryReadScope {
                    bot_id: BOT,
                    telegram_user_id: OWNER,
                    internal_user_id: uuid::Uuid::now_v7(),
                    chat_id: OWNER,
                },
                analysis_id: uuid::Uuid::now_v7(),
                expires_at: T0 + 10,
            },
            T0,
        )
        .await
        .expect("library read token");

    let counts = test
        .database
        .cleanup_interactions(T0 + 10, T0, 10)
        .await
        .expect("cleanup");
    assert_eq!(counts.tokens_deleted, 1);
    let remaining: i64 =
        sqlx::query_scalar("select count(*) from telegram.interaction_tokens where token = $1")
            .bind(token)
            .fetch_one(test.pool())
            .await
            .expect("remaining token count");
    assert_eq!(remaining, 0);
}

fn dialogue_scope() -> DialogueScope {
    DialogueScope {
        bot_id: BOT,
        telegram_user_id: OWNER,
        chat_id: OWNER,
    }
}

fn token_scope() -> TokenScope {
    TokenScope {
        bot_id: BOT,
        telegram_user_id: OWNER,
        chat_id: OWNER,
        message_id: None,
    }
}

fn dialogue_payload() -> GitHubRepositoryDialogue {
    serde_json::from_value(serde_json::json!({
        "target": {
            "github_repository_numeric_id": 42,
            "repository_full_name": "owner/repository",
            "canonical_url": "https://github.com/owner/repository"
        },
        "account_ref": "github-account:018f0000-0000-7000-8000-000000000604"
    }))
    .expect("closed dialogue payload")
}

async fn create_dialogue(test: &TestDatabase, expires_at: i64) -> uuid::Uuid {
    test.database
        .create_github_dialogue(
            &NewGitHubDialogue {
                scope: dialogue_scope(),
                payload: dialogue_payload(),
                expires_at,
            },
            T0,
        )
        .await
        .expect("dialogue creation")
}

async fn issue_token(test: &TestDatabase, operation_id: uuid::Uuid, expires_at: i64) -> String {
    test.database
        .issue_operation_intent(
            &NewOperationIntent {
                scope: token_scope(),
                operation_id,
                payload: OperationIntentPayload {
                    source_url: Some("https://example.test/article".to_owned()),
                    metadata: None,
                },
                expires_at,
            },
            T0,
        )
        .await
        .expect("token authority")
}

async fn cancel_at(test: &TestDatabase, dialogue_id: uuid::Uuid, terminal_at: i64) {
    sqlx::query(
        "update telegram.dialog_states
         set lifecycle = 'cancelled', terminal_at = to_timestamp($2), updated_at = to_timestamp($2)
         where id = $1",
    )
    .bind(dialogue_id)
    .bind(terminal_at)
    .execute(test.pool())
    .await
    .expect("terminal fixture");
}

async fn create_followed_expiring_token(test: &TestDatabase) -> (uuid::Uuid, String) {
    let operation_id = uuid::Uuid::now_v7();
    test.database
        .record_send_acknowledged(BOT, operation_id, OWNER, 76, T0)
        .await
        .expect("followed operation binding");
    let token = issue_token(test, operation_id, T0 + 10).await;
    (operation_id, token)
}

async fn assert_follow_owner_survives(test: &TestDatabase, operation_id: uuid::Uuid, token: &str) {
    let rows: i64 =
        sqlx::query_scalar("select count(*) from telegram.interaction_tokens where token = $1")
            .bind(token)
            .fetch_one(test.pool())
            .await
            .expect("followed token count");
    assert_eq!(
        rows, 1,
        "operation ownership must survive while its binding is non-terminal"
    );
    assert_eq!(
        test.database
            .find_operation_intent_owner(operation_id)
            .await
            .expect("follow owner lookup"),
        Some(OWNER)
    );
}

#[tokio::test]
async fn cleanup_expires_dialogues_and_removes_only_eligible_tokens_in_one_bounded_batch() {
    let test = TestDatabase::create().await.expect("database");
    let expired_dialogue = create_dialogue(&test, T0 + 10).await;
    let live_dialogue = create_dialogue(&test, T0 + 200).await;
    let old_terminal_dialogue = create_dialogue(&test, T0 + 200).await;
    cancel_at(&test, old_terminal_dialogue, T0 + 1).await;

    let expired_token = issue_token(&test, uuid::Uuid::now_v7(), T0 + 10).await;
    let consumed_token = issue_token(&test, uuid::Uuid::now_v7(), T0 + 200).await;
    test.database
        .consume_interaction_token(TokenPresentation {
            token: &consumed_token,
            surface: TokenSurface::DeepLink,
            scope: token_scope(),
            now: T0 + 1,
        })
        .await
        .expect("consume authority")
        .expect("released authority");

    let (followed_operation, followed_token) = create_followed_expiring_token(&test).await;

    let live_operation = uuid::Uuid::now_v7();
    test.database
        .record_send_acknowledged(BOT, live_operation, OWNER, 77, T0)
        .await
        .expect("message binding");
    let binding_before = test
        .database
        .find_binding(live_operation, OWNER)
        .await
        .expect("binding read")
        .expect("binding");
    let live_token = issue_token(&test, live_operation, T0 + 200).await;

    let counts = test
        .database
        .cleanup_interactions(T0 + 10, T0 + 5, 10)
        .await
        .expect("cleanup");

    assert!(counts.dialogues_expired <= 10);
    assert!(counts.tokens_deleted <= 10);
    assert!(counts.dialogues_deleted <= 10);
    assert_eq!(counts.dialogues_expired, 1);
    assert_eq!(counts.tokens_deleted, 2);
    assert_eq!(counts.dialogues_deleted, 1);

    let expired = test
        .database
        .find_github_dialogue(expired_dialogue, dialogue_scope())
        .await
        .expect("expired dialogue read")
        .expect("expired dialogue retained");
    assert_eq!(expired.lifecycle, DialogueLifecycle::Expired);
    assert_eq!(expired.version, 1);
    let live = test
        .database
        .find_github_dialogue(live_dialogue, dialogue_scope())
        .await
        .expect("live dialogue read")
        .expect("live dialogue retained");
    assert_eq!(live.lifecycle, DialogueLifecycle::Active);
    assert!(
        test.database
            .find_github_dialogue(old_terminal_dialogue, dialogue_scope())
            .await
            .expect("old terminal dialogue read")
            .is_none()
    );

    let remaining: Vec<String> = sqlx::query_scalar(
        "select token from telegram.interaction_tokens
         where token = any($1) order by token",
    )
    .bind(vec![
        expired_token.clone(),
        consumed_token.clone(),
        live_token.clone(),
    ])
    .fetch_all(test.pool())
    .await
    .expect("remaining token read");
    assert_eq!(remaining, vec![live_token.clone()]);
    assert_follow_owner_survives(&test, followed_operation, &followed_token).await;
    test.database
        .consume_interaction_token(TokenPresentation {
            token: &live_token,
            surface: TokenSurface::DeepLink,
            scope: token_scope(),
            now: T0 + 11,
        })
        .await
        .expect("live token presentation")
        .expect("live authority remains consumable");

    let binding_after = test
        .database
        .find_binding(live_operation, OWNER)
        .await
        .expect("binding read")
        .expect("binding retained");
    assert_eq!(binding_after, binding_before);
}
