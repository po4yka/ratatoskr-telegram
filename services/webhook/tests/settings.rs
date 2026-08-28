//! Deterministic `/settings` interaction over the real authorization and durable queue.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "assertions in a test binary"
)]

use axum::body::Body;
use http::Request;
use ratatoskr_telegram_webhook::intake::{self, Intake, IntakeSettings};
use secrecy::SecretString;
use serde_json::json;
use sqlx::Row as _;
use telegram_persistence::test_support::TestDatabase;
use telegram_persistence::{IdentityProfile, QuietPolicy};
use tower::ServiceExt as _;

const BOT_ID: i64 = 700_100_200;
const OWNER: i64 = 900_700_601;
const SECRET: &str = "webhook-secret-0123456789abcdef";

fn update(id: i64, sender: i64, chat: i64, text: &str) -> serde_json::Value {
    json!({
        "update_id": id,
        "message": {
            "message_id": id,
            "from": {"id": sender, "is_bot": false, "first_name": "Synthetic"},
            "date": 1_760_000_000_i64,
            "chat": {"id": chat, "type": "private", "first_name": "Synthetic"},
            "text": text,
        }
    })
}

async fn deliver(
    intake: &std::sync::Arc<Intake>,
    receiver: &mut tokio::sync::mpsc::Receiver<intake::QueuedUpdate>,
    value: serde_json::Value,
) {
    let response = intake
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("content-type", "application/json")
                .header("x-telegram-bot-api-secret-token", SECRET)
                .body(Body::from(value.to_string()))
                .expect("request"),
        )
        .await
        .expect("router response");
    assert!(response.status().is_success());
    let queued = receiver.recv().await.expect("accepted update is queued");
    intake::process_one(&intake.database, &queued, None).await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one interaction test preserves the exact command sequence and sibling-state checks"
)]
async fn settings_commands_inspect_and_update_notification_policy() {
    let database = TestDatabase::create().await.expect("database");
    database
        .database
        .ensure_identity(OWNER, &IdentityProfile::default())
        .await
        .expect("owner identity");
    let (intake, mut receiver) = Intake::new(
        IntakeSettings {
            secret: SecretString::new(SECRET.into()),
            max_body_bytes: 4096,
            bot_id: BOT_ID,
            queue_capacity: 16,
        },
        database.database.clone(),
    );

    deliver(&intake, &mut receiver, update(1, OWNER, OWNER, "/settings")).await;
    deliver(
        &intake,
        &mut receiver,
        update(2, OWNER, OWNER, "/settings notification backup_outcome off"),
    )
    .await;
    deliver(
        &intake,
        &mut receiver,
        update(3, OWNER, OWNER, "/settings quiet-hours 23:00-07:00"),
    )
    .await;
    deliver(
        &intake,
        &mut receiver,
        update(4, OWNER, OWNER, "/settings high-priority-bypass on"),
    )
    .await;
    deliver(
        &intake,
        &mut receiver,
        update(5, OWNER, OWNER, "/settings notifications off"),
    )
    .await;

    let preference = database
        .database
        .notification_preferences(OWNER, OWNER)
        .await
        .expect("read")
        .expect("default exists after authorization");
    assert!(!preference.enabled);
    assert_eq!(preference.class_enabled("backup_outcome"), Some(false));
    assert_eq!(preference.class_enabled("analysis_ready"), None);
    assert_eq!(
        preference.quiet_policy,
        QuietPolicy::Custom {
            start_minute: 23 * 60,
            end_minute: 7 * 60,
        }
    );
    assert!(preference.high_priority_bypass);

    let before = preference.clone();
    deliver(
        &intake,
        &mut receiver,
        update(6, OWNER, OWNER, "/settings quiet-hours 24:00-07:00"),
    )
    .await;
    assert_eq!(
        database
            .database
            .notification_preferences(OWNER, OWNER)
            .await
            .expect("read")
            .expect("preference"),
        before,
        "malformed form changes nothing"
    );

    deliver(
        &intake,
        &mut receiver,
        update(
            7,
            OWNER,
            OWNER,
            "/settings notification backup_outcome inherit",
        ),
    )
    .await;
    let inherited = database
        .database
        .notification_preferences(OWNER, OWNER)
        .await
        .expect("read")
        .expect("preference");
    assert_eq!(inherited.class_enabled("backup_outcome"), None);

    let payloads: Vec<serde_json::Value> = sqlx::query(
        "select payload from telegram.outbound_jobs where chat_id = $1 order by created_at, id",
    )
    .bind(OWNER)
    .fetch_all(database.pool())
    .await
    .expect("settings replies")
    .into_iter()
    .map(|row| row.get("payload"))
    .collect();
    assert_eq!(payloads.len(), 7);
    assert!(payloads.iter().all(|payload| {
        payload["text"]
            .as_str()
            .is_some_and(|text| !text.contains("Synthetic"))
    }));

    database.cleanup().await.expect("cleanup");
}
