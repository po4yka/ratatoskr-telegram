//! Opaque, expiring, scope-bound interaction authority.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use telegram_persistence::interaction_tokens::{
    NewOperationIntent, OperationIntentPayload, ReleasedAction, ReleasedToken, TokenPresentation,
    TokenRefusal, TokenScope, TokenSurface,
};
use telegram_persistence::test_support::TestDatabase;

const T0: i64 = 1_800_000_000;
const BOT: i64 = 42;
const OWNER: i64 = 900_700_601;

async fn issue_deep_link(test: &TestDatabase, operation_id: uuid::Uuid, expires_at: i64) -> String {
    test.database
        .issue_operation_intent(
            &NewOperationIntent {
                scope: TokenScope {
                    bot_id: BOT,
                    telegram_user_id: OWNER,
                    chat_id: OWNER,
                    message_id: None,
                },
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
        .expect("deep-link authority")
}

async fn insert_deep_link(test: &TestDatabase, expires_at: i64) -> String {
    issue_deep_link(test, uuid::Uuid::now_v7(), expires_at).await
}

fn owner_presentation(token: &str, now: i64) -> TokenPresentation<'_> {
    TokenPresentation {
        token,
        surface: TokenSurface::DeepLink,
        scope: TokenScope {
            bot_id: BOT,
            telegram_user_id: OWNER,
            chat_id: OWNER,
            message_id: None,
        },
        now,
    }
}

#[tokio::test]
async fn token_expires_at_its_boundary() {
    let test = TestDatabase::create().await.expect("database");
    let token = insert_deep_link(&test, T0 + 60).await;

    let result = test
        .database
        .consume_interaction_token(owner_presentation(&token, T0 + 60))
        .await
        .expect("presentation");
    assert_eq!(result, Err(TokenRefusal::Expired));

    let consumed: Option<bool> = sqlx::query_scalar(
        "select consumed_at is not null from telegram.interaction_tokens where token = $1",
    )
    .bind(&token)
    .fetch_one(test.pool())
    .await
    .expect("consumption evidence");
    assert_eq!(consumed, Some(false), "expiry must not consume authority");
}

#[tokio::test]
async fn scope_mismatch_preserves_the_owners_live_token() {
    let test = TestDatabase::create().await.expect("database");
    let mismatched_scopes = [
        TokenScope {
            bot_id: BOT + 1,
            telegram_user_id: OWNER,
            chat_id: OWNER,
            message_id: None,
        },
        TokenScope {
            bot_id: BOT,
            telegram_user_id: OWNER + 1,
            chat_id: OWNER,
            message_id: None,
        },
        TokenScope {
            bot_id: BOT,
            telegram_user_id: OWNER,
            chat_id: OWNER + 1,
            message_id: None,
        },
        TokenScope {
            bot_id: BOT,
            telegram_user_id: OWNER,
            chat_id: OWNER,
            message_id: Some(77),
        },
    ];

    for scope in mismatched_scopes {
        let token = insert_deep_link(&test, T0 + 60).await;
        let foreign = test
            .database
            .consume_interaction_token(TokenPresentation {
                token: &token,
                surface: TokenSurface::DeepLink,
                scope,
                now: T0 + 1,
            })
            .await
            .expect("foreign presentation");
        assert_eq!(foreign, Err(TokenRefusal::ScopeMismatch));

        let owner = test
            .database
            .consume_interaction_token(owner_presentation(&token, T0 + 2))
            .await
            .expect("owner presentation");
        assert!(
            owner.is_ok(),
            "scope mismatch must preserve owner authority"
        );
    }
}

#[tokio::test]
async fn concurrent_single_use_presentations_have_one_winner() {
    let test = TestDatabase::create().await.expect("database");
    let token = insert_deep_link(&test, T0 + 60).await;
    let first_database = test.database.clone();
    let second_database = test.database.clone();
    let (first, second) = tokio::join!(
        first_database.consume_interaction_token(owner_presentation(&token, T0 + 1)),
        second_database.consume_interaction_token(owner_presentation(&token, T0 + 1)),
    );
    let results = [
        first.expect("first presentation"),
        second.expect("second presentation"),
    ];
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "one one-time action must be released",
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Err(TokenRefusal::Consumed))
            .count(),
        1,
        "the losing presentation must observe consumption",
    );

    let consumer: Option<i64> = sqlx::query_scalar(
        "select consumed_by_user from telegram.interaction_tokens where token = $1",
    )
    .bind(&token)
    .fetch_one(test.pool())
    .await
    .expect("consumption evidence");
    assert_eq!(consumer, Some(OWNER));
}

#[tokio::test]
async fn deep_link_intent_resolves_once_for_its_bound_owner() {
    let test = TestDatabase::create().await.expect("database");
    let operation_id = uuid::Uuid::now_v7();
    test.database
        .ensure_operation_binding(BOT, operation_id, OWNER)
        .await
        .expect("operation binding");
    let token = issue_deep_link(&test, operation_id, T0 + 60).await;

    let stored = test
        .database
        .find_live_operation_intent_by_operation(operation_id, T0 + 1)
        .await
        .expect("intent lookup")
        .expect("live operation intent");
    assert_eq!(stored.token, token);
    assert_eq!(stored.operation_id, operation_id);

    let expected = ReleasedToken {
        action: ReleasedAction::OperationStatus,
        operation_id,
        payload: OperationIntentPayload {
            source_url: Some("https://example.test/article".to_owned()),
            metadata: None,
        },
    };
    let resolved = test
        .database
        .resolve_operation_intent(owner_presentation(&token, T0 + 2))
        .await
        .expect("owner resolution");
    assert_eq!(resolved, Ok(expected));
    let replay = test
        .database
        .resolve_operation_intent(owner_presentation(&token, T0 + 3))
        .await
        .expect("replay resolution");
    assert_eq!(replay, Err(TokenRefusal::Consumed));

    let binding = test
        .database
        .find_binding(operation_id, OWNER)
        .await
        .expect("binding read")
        .expect("operation binding remains");
    assert_eq!(binding.operation_id, operation_id);
    assert_eq!(binding.message_id, None);
}
