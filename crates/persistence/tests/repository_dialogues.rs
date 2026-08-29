//! Repository dialogue schema and transactional confirmation authority.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use ratatoskr_github_contracts::RepositoryActionCapability;
use telegram_persistence::dialogues::{CallbackRefusal, DecisionTransition, ReleasingUpdate};
use telegram_persistence::test_support::TestDatabase;
use telegram_persistence::{AdmittedUpdate, RecordOutcome, UpdateState};

const T0: i64 = 1_800_000_000;
const BOT: i64 = 42;
const OWNER: i64 = 900_700_601;
const CHAT: i64 = OWNER;
const RELEASING_UPDATE: i64 = 70_001;
const FOREIGN_UPDATE: i64 = 70_002;

fn preview() -> ratatoskr_github_contracts::RepositoryPreviewResponse {
    serde_json::from_value(serde_json::json!({
        "target": {"github_repository_numeric_id": 42, "repository_full_name": "owner/repository", "canonical_url": "https://github.com/owner/repository"},
        "description": "A tool", "stargazer_count": 42, "primary_language": "Rust",
        "account_ref": "github-account:018f0000-0000-7000-8000-000000000604",
        "available_actions": ["metadata", "track", "star"]
    })).expect("preview contract")
}

async fn admit_callback_update(test: &TestDatabase, update_id: i64) {
    assert_eq!(
        test.database
            .record_update(&AdmittedUpdate {
                bot_id: BOT,
                update_id,
                kind: "callback_query".to_owned(),
                payload: serde_json::json!({"update_id": update_id}).to_string(),
            })
            .await
            .expect("admit callback update"),
        RecordOutcome::Inserted
    );
    test.database
        .settle_update(BOT, update_id, UpdateState::Processing)
        .await
        .expect("claim callback update");
}

#[tokio::test]
async fn submitting_dialogue_records_releasing_update() {
    let test = TestDatabase::create().await.expect("database");
    admit_callback_update(&test, RELEASING_UPDATE).await;

    let dialogue = test
        .database
        .create_repository_preview_dialogue(BOT, OWNER, CHAT, &preview(), T0, T0 + 900)
        .await
        .expect("dialogue");
    test.database
        .stamp_callback_message(dialogue.dialogue_id, BOT, CHAT, 100, T0 + 1)
        .await
        .expect("stamp selection");
    let star = dialogue
        .selections
        .iter()
        .find(|selection| selection.mode == RepositoryActionCapability::Star)
        .expect("star selection");
    let selected = test
        .database
        .consume_repository_selection(&star.token, BOT, OWNER, CHAT, 100, T0 + 2)
        .await
        .expect("selection")
        .expect("selection accepted");
    test.database
        .stamp_callback_message(dialogue.dialogue_id, BOT, CHAT, 101, T0 + 3)
        .await
        .expect("stamp confirmation");
    let decision = test
        .database
        .consume_repository_decision(
            &selected.confirm_token,
            ReleasingUpdate {
                bot_id: BOT,
                update_id: RELEASING_UPDATE,
            },
            OWNER,
            CHAT,
            101,
            T0 + 4,
        )
        .await
        .expect("decision")
        .expect("confirmation accepted");
    assert!(matches!(decision, DecisionTransition::Confirmed(_)));

    let authority: (Option<i64>, Option<i64>) = sqlx::query_as(
        "select (to_jsonb(dialogue)->>'releasing_bot_id')::bigint,
                (to_jsonb(dialogue)->>'releasing_update_id')::bigint
         from telegram.dialog_states dialogue where id = $1",
    )
    .bind(dialogue.dialogue_id)
    .fetch_one(test.pool())
    .await
    .expect("submitting authority");
    assert_eq!(
        authority,
        (Some(BOT), Some(RELEASING_UPDATE)),
        "the submitting transition must retain the exact admitted update authority"
    );
}

#[tokio::test]
async fn foreign_update_cannot_resume_consumed_confirmation() {
    let test = TestDatabase::create().await.expect("database");
    for update_id in [RELEASING_UPDATE, FOREIGN_UPDATE] {
        admit_callback_update(&test, update_id).await;
    }

    let dialogue = test
        .database
        .create_repository_preview_dialogue(BOT, OWNER, CHAT, &preview(), T0, T0 + 900)
        .await
        .expect("dialogue");
    test.database
        .stamp_callback_message(dialogue.dialogue_id, BOT, CHAT, 100, T0 + 1)
        .await
        .expect("stamp selection");
    let star = dialogue
        .selections
        .iter()
        .find(|selection| selection.mode == RepositoryActionCapability::Star)
        .expect("star selection");
    let selected = test
        .database
        .consume_repository_selection(&star.token, BOT, OWNER, CHAT, 100, T0 + 2)
        .await
        .expect("selection")
        .expect("selection accepted");
    test.database
        .stamp_callback_message(dialogue.dialogue_id, BOT, CHAT, 101, T0 + 3)
        .await
        .expect("stamp confirmation");
    let confirmed = test
        .database
        .consume_repository_decision(
            &selected.confirm_token,
            ReleasingUpdate {
                bot_id: BOT,
                update_id: RELEASING_UPDATE,
            },
            OWNER,
            CHAT,
            101,
            T0 + 4,
        )
        .await
        .expect("initial decision")
        .expect("initial confirmation accepted");
    let DecisionTransition::Confirmed(action) = confirmed else {
        panic!("confirmation must release the action");
    };

    let foreign = test
        .database
        .consume_repository_decision(
            &selected.confirm_token,
            ReleasingUpdate {
                bot_id: BOT,
                update_id: FOREIGN_UPDATE,
            },
            OWNER,
            CHAT,
            101,
            T0 + 5,
        )
        .await
        .expect("foreign replay");
    assert_eq!(
        foreign,
        Err(CallbackRefusal::Consumed),
        "a different durable update must inherit no consumed confirmation authority"
    );

    let original = test
        .database
        .consume_repository_decision(
            &selected.confirm_token,
            ReleasingUpdate {
                bot_id: BOT,
                update_id: RELEASING_UPDATE,
            },
            OWNER,
            CHAT,
            101,
            T0 + 5,
        )
        .await
        .expect("original update recovery");
    assert_eq!(
        original,
        Ok(DecisionTransition::Confirmed(action)),
        "only the durable update recorded by the submitting transition may resume"
    );
}

#[tokio::test]
async fn schema_and_store_enforce_owner_message_expiry_version_and_one_winner() {
    let test = TestDatabase::create().await.expect("database");
    let update = ReleasingUpdate {
        bot_id: BOT,
        update_id: 70_010,
    };
    let competing_update = ReleasingUpdate {
        bot_id: BOT,
        update_id: 70_012,
    };
    admit_callback_update(&test, update.update_id).await;
    admit_callback_update(&test, competing_update.update_id).await;
    let dialogue = test
        .database
        .create_repository_preview_dialogue(BOT, OWNER, CHAT, &preview(), T0, T0 + 900)
        .await
        .expect("dialogue");
    assert_eq!(dialogue.selections.len(), 3);
    assert!(
        dialogue
            .selections
            .iter()
            .all(|selection| selection.token.len() == 64)
    );
    assert!(
        dialogue
            .selections
            .iter()
            .all(|selection| !selection.token.contains("github"))
    );
    let star = dialogue
        .selections
        .iter()
        .find(|item| item.mode == RepositoryActionCapability::Star)
        .expect("star");

    assert!(
        test.database
            .stamp_callback_message(dialogue.dialogue_id, BOT, CHAT, 100, T0 + 1)
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
            .stamp_callback_message(dialogue.dialogue_id, BOT, CHAT, 101, T0 + 4)
            .await
            .expect("stamp confirm")
    );
    let first_database = test.database.clone();
    let second_database = test.database.clone();
    let first_token = selected.confirm_token.clone();
    let second_token = selected.confirm_token.clone();
    let (first, second) = tokio::join!(
        first_database.consume_repository_decision(&first_token, update, OWNER, CHAT, 101, T0 + 5),
        second_database.consume_repository_decision(
            &second_token,
            competing_update,
            OWNER,
            CHAT,
            101,
            T0 + 5
        )
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
        .consume_repository_decision(&selected.cancel_token, update, OWNER, CHAT, 101, T0 + 5)
        .await
        .expect("cancel race");
    assert_eq!(cancel_lost, Err(CallbackRefusal::Invalid));
}

#[tokio::test]
async fn expired_selection_never_advances() {
    let test = TestDatabase::create().await.expect("database");
    let dialogue = test
        .database
        .create_repository_preview_dialogue(BOT, OWNER, CHAT, &preview(), T0, T0 + 10)
        .await
        .expect("dialogue");
    test.database
        .stamp_callback_message(dialogue.dialogue_id, BOT, CHAT, 100, T0 + 1)
        .await
        .expect("stamp");
    let expired = test
        .database
        .consume_repository_selection(
            &dialogue.selections[0].token,
            BOT,
            OWNER,
            CHAT,
            100,
            T0 + 10,
        )
        .await
        .expect("expired");
    assert_eq!(expired, Err(CallbackRefusal::Expired));
}

#[tokio::test]
async fn confirmed_metadata_never_carries_the_star_account_reference() {
    let test = TestDatabase::create().await.expect("database");
    let update = ReleasingUpdate {
        bot_id: BOT,
        update_id: 70_011,
    };
    admit_callback_update(&test, update.update_id).await;
    let dialogue = test
        .database
        .create_repository_preview_dialogue(BOT, OWNER, CHAT, &preview(), T0, T0 + 900)
        .await
        .expect("dialogue");
    test.database
        .stamp_callback_message(dialogue.dialogue_id, BOT, CHAT, 100, T0 + 1)
        .await
        .expect("stamp");
    let metadata = dialogue
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
        .stamp_callback_message(dialogue.dialogue_id, BOT, CHAT, 101, T0 + 3)
        .await
        .expect("stamp confirmation");
    let decision = test
        .database
        .consume_repository_decision(&selected.confirm_token, update, OWNER, CHAT, 101, T0 + 4)
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
