//! Notification preference persistence, including optimistic writes and schema-equivalent guards.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use sqlx::Row as _;
use telegram_persistence::bindings::IdentityProfile;
use telegram_persistence::test_support::TestDatabase;

#[tokio::test]
async fn preference_write_is_versioned_and_atomic() {
    let test = TestDatabase::create().await.expect("disposable database");
    let user_id = uuid::Uuid::now_v7();
    test.database
        .ensure_identity(700_100_200, &IdentityProfile::default())
        .await
        .expect("identity");
    sqlx::query("update telegram.identities set internal_user_id = $1 where telegram_user_id = $2")
        .bind(user_id)
        .bind(700_100_200_i64)
        .execute(test.pool())
        .await
        .expect("internal binding");
    test.database.ensure_chat(900_700_600).await.expect("chat");
    test.database
        .bind_private_chat(700_100_200, 900_700_600)
        .await
        .expect("private binding");

    let row = sqlx::query(
        "select enabled, quiet_policy, high_priority_bypass, version
         from telegram.notification_preferences
         where telegram_user_id = 700100200 and chat_id = 900700600",
    )
    .fetch_one(test.pool())
    .await
    .expect("binding creates default preferences");
    assert!(row.get::<bool, _>("enabled"));
    assert_eq!(row.get::<&str, _>("quiet_policy"), "inherit");
    assert!(!row.get::<bool, _>("high_priority_bypass"));
    assert_eq!(row.get::<i64, _>("version"), 0);

    // One optimistic write changes the selected class and global policy together.
    let changed = test
        .database
        .update_notification_preferences(
            700_100_200,
            900_700_600,
            0,
            &telegram_persistence::NotificationPreferenceUpdate {
                enabled: true,
                quiet_policy: telegram_persistence::QuietPolicy::Custom {
                    start_minute: 1_320,
                    end_minute: 420,
                },
                high_priority_bypass: true,
                class_override: Some(("backup_outcome".to_owned(), Some(false))),
            },
            1_800_000_000,
        )
        .await
        .expect("fresh write");
    assert_eq!(changed.version, 1);
    assert_eq!(changed.class_enabled("backup_outcome"), Some(false));

    let stale = test
        .database
        .update_notification_preferences(
            700_100_200,
            900_700_600,
            0,
            &telegram_persistence::NotificationPreferenceUpdate {
                enabled: false,
                quiet_policy: telegram_persistence::QuietPolicy::Disabled,
                high_priority_bypass: false,
                class_override: None,
            },
            1_800_000_001,
        )
        .await;
    assert!(matches!(
        stale,
        Err(telegram_persistence::PersistenceError::StalePreference)
    ));

    let malformed = sqlx::query(
        "update telegram.notification_preferences
         set quiet_policy = 'custom', quiet_start_minute = 10, quiet_end_minute = 10
         where telegram_user_id = 700100200 and chat_id = 900700600",
    )
    .execute(test.pool())
    .await;
    assert!(
        malformed.is_err(),
        "direct SQL cannot bypass quiet-window guards"
    );

    let foreign = sqlx::query(
        "insert into telegram.notification_preferences (telegram_user_id, chat_id)
         values (700100201, 900700600)",
    )
    .execute(test.pool())
    .await;
    assert!(
        foreign.is_err(),
        "an unbound actor cannot own chat preferences"
    );

    test.cleanup().await.expect("cleanup");
}
