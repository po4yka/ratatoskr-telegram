//! Worker-owned cleanup scheduling.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use std::time::{SystemTime, UNIX_EPOCH};

use ratatoskr_telegram_webhook::intake;
use telegram_persistence::dialogues::{
    DialogueLifecycle, DialogueScope, GitHubRepositoryDialogue, NewGitHubDialogue,
};
use telegram_persistence::test_support::TestDatabase;

const BOT: i64 = 42;
const OWNER: i64 = 900_700_601;

fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_secs(),
    )
    .expect("current time fits i64")
}

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

#[tokio::test]
async fn worker_runs_cleanup_on_startup() {
    let test = TestDatabase::create().await.expect("database");
    let now = now_secs();
    let dialogue_id = test
        .database
        .create_github_dialogue(
            &NewGitHubDialogue {
                scope: scope(),
                payload: payload(),
                expires_at: now - 1,
            },
            now - 10,
        )
        .await
        .expect("stale dialogue");
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    drop(sender);

    intake::run_worker(test.database.clone(), receiver, None).await;

    let dialogue = test
        .database
        .find_github_dialogue(dialogue_id, scope())
        .await
        .expect("dialogue read")
        .expect("expired dialogue retained");
    assert_eq!(dialogue.lifecycle, DialogueLifecycle::Expired);
    assert_eq!(dialogue.version, 1);
}
