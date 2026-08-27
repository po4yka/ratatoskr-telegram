//! Callback-flow schema and transactional confirmation authority.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use ratatoskr_github_contracts::RepositoryActionCapability;
use telegram_persistence::callback_flows::{CallbackRefusal, DecisionTransition};
use telegram_persistence::test_support::TestDatabase;

const T0: i64 = 1_800_000_000;
const BOT: i64 = 42;
const OWNER: i64 = 900_700_601;
const CHAT: i64 = OWNER;

fn preview() -> ratatoskr_github_contracts::RepositoryPreviewResponse {
    serde_json::from_value(serde_json::json!({
        "target": {"github_repository_numeric_id": 42, "repository_full_name": "owner/repository", "canonical_url": "https://github.com/owner/repository"},
        "description": "A tool", "stargazer_count": 42, "primary_language": "Rust",
        "account_ref": "github-account:018f0000-0000-7000-8000-000000000604",
        "available_actions": ["metadata", "track", "star"]
    })).expect("preview contract")
}

#[tokio::test]
async fn schema_and_store_enforce_owner_message_expiry_version_and_one_winner() {
    let test = TestDatabase::create().await.expect("database");
    let flow = test
        .database
        .create_repository_preview_flow(BOT, OWNER, CHAT, &preview(), T0, T0 + 900)
        .await
        .expect("flow");
    assert_eq!(flow.selections.len(), 3);
    assert!(
        flow.selections
            .iter()
            .all(|selection| selection.token.len() <= 64)
    );
    assert!(
        flow.selections
            .iter()
            .all(|selection| !selection.token.contains("github"))
    );
    let star = flow
        .selections
        .iter()
        .find(|item| item.mode == RepositoryActionCapability::Star)
        .expect("star");

    assert!(
        test.database
            .stamp_callback_message(flow.flow_id, BOT, CHAT, 100, T0 + 1)
            .await
            .expect("stamp")
    );
    let foreign = test
        .database
        .consume_repository_selection(&star.token, BOT, OWNER + 1, CHAT, 100, T0 + 2)
        .await
        .expect("foreign");
    assert_eq!(foreign, Err(CallbackRefusal::Invalid));

    let selected = test
        .database
        .consume_repository_selection(&star.token, BOT, OWNER, CHAT, 100, T0 + 2)
        .await
        .expect("selection")
        .expect("accepted");
    assert_eq!(selected.mode, RepositoryActionCapability::Star);
    let replay = test
        .database
        .consume_repository_selection(&star.token, BOT, OWNER, CHAT, 100, T0 + 3)
        .await
        .expect("replay");
    assert_eq!(replay, Err(CallbackRefusal::Consumed));

    assert!(
        test.database
            .stamp_callback_message(flow.flow_id, BOT, CHAT, 101, T0 + 4)
            .await
            .expect("stamp confirm")
    );
    let first_database = test.database.clone();
    let second_database = test.database.clone();
    let first_token = selected.confirm_token.clone();
    let second_token = selected.confirm_token.clone();
    let (first, second) = tokio::join!(
        first_database.consume_repository_decision(&first_token, BOT, OWNER, CHAT, 101, T0 + 5),
        second_database.consume_repository_decision(&second_token, BOT, OWNER, CHAT, 101, T0 + 5)
    );
    let first = first.expect("first concurrent decision");
    let second = second.expect("second concurrent decision");
    let winners = [&first, &second]
        .into_iter()
        .filter(|result| matches!(result, Ok(DecisionTransition::Confirmed(_))))
        .count();
    assert_eq!(winners, 1, "exactly one concurrent presentation wins");
    let cancel_lost = test
        .database
        .consume_repository_decision(&selected.cancel_token, BOT, OWNER, CHAT, 101, T0 + 5)
        .await
        .expect("cancel race");
    assert_eq!(cancel_lost, Err(CallbackRefusal::Invalid));
}

#[tokio::test]
async fn expired_selection_never_advances() {
    let test = TestDatabase::create().await.expect("database");
    let flow = test
        .database
        .create_repository_preview_flow(BOT, OWNER, CHAT, &preview(), T0, T0 + 10)
        .await
        .expect("flow");
    test.database
        .stamp_callback_message(flow.flow_id, BOT, CHAT, 100, T0 + 1)
        .await
        .expect("stamp");
    let expired = test
        .database
        .consume_repository_selection(&flow.selections[0].token, BOT, OWNER, CHAT, 100, T0 + 10)
        .await
        .expect("expired");
    assert_eq!(expired, Err(CallbackRefusal::Expired));
}

#[tokio::test]
async fn confirmed_metadata_never_carries_the_star_account_reference() {
    let test = TestDatabase::create().await.expect("database");
    let flow = test
        .database
        .create_repository_preview_flow(BOT, OWNER, CHAT, &preview(), T0, T0 + 900)
        .await
        .expect("flow");
    test.database
        .stamp_callback_message(flow.flow_id, BOT, CHAT, 100, T0 + 1)
        .await
        .expect("stamp");
    let metadata = flow
        .selections
        .iter()
        .find(|item| item.mode == RepositoryActionCapability::Metadata)
        .expect("metadata token");
    let selected = test
        .database
        .consume_repository_selection(&metadata.token, BOT, OWNER, CHAT, 100, T0 + 2)
        .await
        .expect("selection")
        .expect("accepted");
    test.database
        .stamp_callback_message(flow.flow_id, BOT, CHAT, 101, T0 + 3)
        .await
        .expect("stamp confirmation");
    let decision = test
        .database
        .consume_repository_decision(&selected.confirm_token, BOT, OWNER, CHAT, 101, T0 + 4)
        .await
        .expect("decision")
        .expect("confirmed");
    let DecisionTransition::Confirmed(action) = decision else {
        panic!("confirmation must win");
    };
    assert_eq!(action.mode, RepositoryActionCapability::Metadata);
    assert!(
        action.account_ref.is_none(),
        "non-star actions must satisfy the shared contract"
    );
}
