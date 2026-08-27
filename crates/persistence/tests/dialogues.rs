//! Restart-safe dialogue state and optimistic transitions.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use secrecy::SecretString;
use telegram_core::DatabaseConfig;
use telegram_persistence::Database;
use telegram_persistence::dialogues::{
    DialogueLifecycle, DialogueRefusal, DialogueScope, DialogueStep, DialogueTransition,
    GitHubRepositoryDialogue, NewGitHubDialogue,
};
use telegram_persistence::test_support::TestDatabase;

const T0: i64 = 1_800_000_000;
const BOT: i64 = 42;
const OWNER: i64 = 900_700_601;

fn scope() -> DialogueScope {
    DialogueScope {
        bot_id: BOT,
        telegram_user_id: OWNER,
        chat_id: OWNER,
    }
}

fn payload() -> GitHubRepositoryDialogue {
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

fn new_dialogue() -> NewGitHubDialogue {
    NewGitHubDialogue {
        scope: scope(),
        payload: payload(),
        expires_at: T0 + 900,
    }
}

#[tokio::test]
async fn awaiting_dialogue_survives_a_new_database_handle() {
    let test = TestDatabase::create().await.expect("database");
    let expected_payload = payload();
    let id = test
        .database
        .create_github_dialogue(
            &NewGitHubDialogue {
                scope: scope(),
                payload: expected_payload.clone(),
                expires_at: T0 + 900,
            },
            T0,
        )
        .await
        .expect("dialogue creation");

    let reopened = Database::connect(&DatabaseConfig {
        url: SecretString::from(test.url()),
        max_connections: 2,
        acquire_timeout_seconds: 5,
    })
    .await
    .expect("new database handle");
    let record = reopened
        .find_github_dialogue(id, scope())
        .await
        .expect("dialogue read")
        .expect("persisted awaiting dialogue");

    assert_eq!(record.scope, scope());
    assert_eq!(record.expected_message_id, None);
    assert_eq!(record.step, DialogueStep::Preview);
    assert_eq!(record.version, 0);
    assert_eq!(record.lifecycle, DialogueLifecycle::Active);
    assert_eq!(record.payload, expected_payload);
    assert_eq!(record.expires_at, T0 + 900);
    reopened.close().await;
}

#[tokio::test]
async fn only_one_expected_version_transition_wins() {
    let test = TestDatabase::create().await.expect("database");
    let id = test
        .database
        .create_github_dialogue(&new_dialogue(), T0)
        .await
        .expect("dialogue creation");
    let transition = DialogueTransition {
        id,
        scope: scope(),
        expected_step: DialogueStep::Preview,
        expected_version: 0,
        next_step: DialogueStep::Confirming,
    };
    let first_database = test.database.clone();
    let second_database = test.database.clone();
    let (first, second) = tokio::join!(
        first_database.transition_github_dialogue(transition, T0 + 1),
        second_database.transition_github_dialogue(transition, T0 + 1),
    );
    let results = [
        first.expect("first transition"),
        second.expect("second transition"),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Err(DialogueRefusal::StaleState))
            .count(),
        1,
    );

    let record = test
        .database
        .find_github_dialogue(id, scope())
        .await
        .expect("dialogue read")
        .expect("dialogue");
    assert_eq!(record.step, DialogueStep::Confirming);
    assert_eq!(record.version, 1);
}

#[tokio::test]
async fn awaiting_input_expires_at_the_timeout_boundary() {
    let test = TestDatabase::create().await.expect("database");
    let id = test
        .database
        .create_github_dialogue(
            &NewGitHubDialogue {
                expires_at: T0 + 10,
                ..new_dialogue()
            },
            T0,
        )
        .await
        .expect("dialogue creation");
    let transition = DialogueTransition {
        id,
        scope: scope(),
        expected_step: DialogueStep::Preview,
        expected_version: 0,
        next_step: DialogueStep::Confirming,
    };

    let result = test
        .database
        .transition_github_dialogue(transition, T0 + 10)
        .await
        .expect("timeout transition");
    assert_eq!(result, Err(DialogueRefusal::Expired));
    let replay = test
        .database
        .transition_github_dialogue(transition, T0 + 11)
        .await
        .expect("terminal replay");
    assert_eq!(replay, Err(DialogueRefusal::Terminal));

    let record = test
        .database
        .find_github_dialogue(id, scope())
        .await
        .expect("dialogue read")
        .expect("dialogue");
    assert_eq!(record.lifecycle, DialogueLifecycle::Expired);
    assert_eq!(record.step, DialogueStep::Preview);
    assert_eq!(record.version, 1, "timeout is one terminal transition");
}
