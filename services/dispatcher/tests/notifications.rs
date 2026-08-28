//! Notification rendering and preference-admission boundary.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use async_nats::jetstream;
use sqlx::Row as _;
use telegram_core::RuntimeRole;
use telegram_persistence::outbound_jobs::DeliveryOutcome;
use telegram_persistence::test_support::TestDatabase;

#[test]
fn notification_renderer_escapes_dynamic_markup_and_omits_private_fields() {
    let notification: ratatoskr_notification_contracts::NotificationRaised =
        serde_json::from_value(serde_json::json!({
            "notification_id": "018f0000-0000-7000-8000-000000000708",
            "class_registry_version": 1,
            "class": "analysis_ready",
            "recipient": "user:018f0000-0000-7000-8000-000000000005",
            "title": "Analysis <ready> & safe",
            "message": "Open > result; token=not-present",
            "operation_ref": "operation:018f0000-0000-7000-8000-000000000302",
            "analysis_ref": "analysis:018f0000-0000-7000-8000-000000000401"
        }))
        .expect("valid contract notification");
    let rendered = ratatoskr_telegram_dispatcher::notifications::render_notification(&notification);
    assert_eq!(rendered.parse_mode.as_deref(), Some("HTML"));
    assert!(
        rendered
            .text
            .contains("<b>Analysis &lt;ready&gt; &amp; safe</b>")
    );
    assert!(
        rendered
            .text
            .contains("Open &gt; result; token=not-present")
    );
    for private in [
        "018f0000-0000-7000-8000-000000000005",
        "018f0000-0000-7000-8000-000000000302",
        "018f0000-0000-7000-8000-000000000401",
    ] {
        assert!(!rendered.text.contains(private), "private reference leaked");
    }
}

#[tokio::test]
#[expect(
    clippy::disallowed_methods,
    reason = "the integration test selects its task-local JetStream fixture, not service config"
)]
async fn dispatcher_requires_matching_preprovisioned_notification_durable() {
    let endpoint = std::env::var("TELEGRAM_TEST_NATS_URL")
        .unwrap_or_else(|_| "nats://127.0.0.1:4222".to_owned());
    let client = async_nats::connect(&endpoint)
        .await
        .expect("the explicitly configured disposable JetStream server");
    let context = jetstream::new(client);
    let config = telegram_core::TelegramConfig::defaults(RuntimeRole::Dispatcher).notification_bus;
    if let Ok(stream) = context.get_stream(&config.stream).await {
        let _ = stream.delete_consumer(&config.durable).await;
        let _ = context.delete_stream(&config.stream).await;
    }
    let stream = context
        .create_stream(jetstream::stream::Config {
            name: config.stream.clone(),
            subjects: vec!["evt.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("fixture stream");

    let missing = ratatoskr_telegram_dispatcher::notifications::lookup_preprovisioned_consumer(
        &context, &config,
    )
    .await;
    assert!(matches!(
        missing,
        Err(ratatoskr_telegram_dispatcher::notifications::NotificationBusError::DurableUnavailable)
    ));

    stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some(config.durable.clone()),
            filter_subject: "evt.platform.foreign.v1".to_owned(),
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ack_wait: std::time::Duration::from_secs(config.ack_wait_seconds),
            ..jetstream::consumer::pull::Config::default()
        })
        .await
        .expect("foreign consumer");
    let mismatched = ratatoskr_telegram_dispatcher::notifications::lookup_preprovisioned_consumer(
        &context, &config,
    )
    .await;
    assert!(matches!(
        mismatched,
        Err(ratatoskr_telegram_dispatcher::notifications::NotificationBusError::DurableMismatch)
    ));

    stream
        .delete_consumer(&config.durable)
        .await
        .expect("remove foreign fixture");
    stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some(config.durable.clone()),
            filter_subject: config.subject.clone(),
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ..jetstream::consumer::pull::Config::default()
        })
        .await
        .expect("matching consumer");
    ratatoskr_telegram_dispatcher::notifications::lookup_preprovisioned_consumer(&context, &config)
        .await
        .expect("matching preprovisioned durable");

    stream
        .delete_consumer(&config.durable)
        .await
        .expect("cleanup consumer");
    context
        .delete_stream(&config.stream)
        .await
        .expect("cleanup stream");
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one test owns the process-global metrics recorder and must observe every bounded outcome"
)]
async fn malformed_notification_evidence_and_metrics_are_content_free() {
    let guard = telegram_telemetry::init(
        &telegram_core::config::TelemetryConfig::default(),
        RuntimeRole::Dispatcher,
    )
    .expect("isolated test binary installs telemetry");
    let database = TestDatabase::create().await.expect("database");
    let user: uuid::Uuid = "018f0000-0000-7000-8000-000000000005"
        .parse()
        .expect("user id");
    sqlx::query(
        "insert into telegram.identities (telegram_user_id, internal_user_id)
         values (700100200, $1)",
    )
    .bind(user)
    .execute(database.pool())
    .await
    .expect("identity");
    sqlx::query("insert into telegram.chats (chat_id, type) values (700100200, 'private')")
        .execute(database.pool())
        .await
        .expect("chat");
    sqlx::query(
        "insert into telegram.private_chat_bindings (telegram_user_id, chat_id, bound_at)
         values (700100200, 700100200, to_timestamp(1800000000))",
    )
    .execute(database.pool())
    .await
    .expect("private binding");
    sqlx::query(
        "insert into telegram.notification_preferences (telegram_user_id, chat_id)
         values (700100200, 700100200)",
    )
    .execute(database.pool())
    .await
    .expect("notification preference");

    let unknown = notification_envelope(
        "018f0000-0000-7000-8000-000000000610",
        "018f0000-0000-7000-8000-000000000710",
        "carrier_pigeon",
    );
    for _ in 0..2 {
        assert_eq!(
            ratatoskr_telegram_dispatcher::notifications::process_payload(
                &database.database,
                700_100_200,
                Some(40),
                &unknown,
                1_800_000_000,
            )
            .await,
            ratatoskr_telegram_dispatcher::notifications::TransportDisposition::Ack
        );
    }
    sqlx::query(
        "update telegram.notification_preferences set enabled = false
         where telegram_user_id = 700100200 and chat_id = 700100200",
    )
    .execute(database.pool())
    .await
    .expect("disable notifications");
    let suppressed = notification_envelope(
        "018f0000-0000-7000-8000-000000000611",
        "018f0000-0000-7000-8000-000000000711",
        "operation_failed",
    );
    assert_eq!(
        ratatoskr_telegram_dispatcher::notifications::process_payload(
            &database.database,
            700_100_200,
            Some(43),
            &suppressed,
            1_800_000_000,
        )
        .await,
        ratatoskr_telegram_dispatcher::notifications::TransportDisposition::Ack
    );
    sqlx::query(
        "update telegram.notification_preferences
         set enabled = true, quiet_policy = 'custom', quiet_start_minute = 400,
             quiet_end_minute = 500
         where telegram_user_id = 700100200 and chat_id = 700100200",
    )
    .execute(database.pool())
    .await
    .expect("quiet hours");
    let deferred = notification_envelope(
        "018f0000-0000-7000-8000-000000000612",
        "018f0000-0000-7000-8000-000000000712",
        "analysis_ready",
    );
    assert_eq!(
        ratatoskr_telegram_dispatcher::notifications::process_payload(
            &database.database,
            700_100_200,
            Some(44),
            &deferred,
            1_800_000_000,
        )
        .await,
        ratatoskr_telegram_dispatcher::notifications::TransportDisposition::Ack
    );

    ratatoskr_telegram_dispatcher::notifications::record_delivery_outcome(
        Some("carrier_pigeon"),
        1,
        5,
        &DeliveryOutcome::Sent,
    );
    ratatoskr_telegram_dispatcher::notifications::record_delivery_outcome(
        Some("analysis_ready"),
        1,
        5,
        &DeliveryOutcome::RetryWithBackoff { delay_secs: 2 },
    );
    ratatoskr_telegram_dispatcher::notifications::record_delivery_outcome(
        Some("operation_failed"),
        5,
        5,
        &DeliveryOutcome::RetryWithBackoff { delay_secs: 2 },
    );
    ratatoskr_telegram_dispatcher::notifications::record_consumer_progress(7);
    let disposition = ratatoskr_telegram_dispatcher::notifications::process_payload(
        &database.database,
        700_100_200,
        Some(41),
        br#"{"private":"body secret"}"#,
        1_800_000_000,
    )
    .await;
    assert_eq!(
        disposition,
        ratatoskr_telegram_dispatcher::notifications::TransportDisposition::Term
    );

    let wrong_type = br#"{
      "event_id":"018f0000-0000-7000-8000-000000000602",
      "event_type":"platform.operation.progressed.v1",
      "occurred_at":"2026-08-25T12:00:00Z",
      "producer":"ratatoskr-platform",
      "aggregate_id":"notification:018f0000-0000-7000-8000-000000000708",
      "correlation_id":"operation:018f0000-0000-7000-8000-000000000302",
      "schema_version":1,
      "payload":{}
    }"#;
    assert_eq!(
        ratatoskr_telegram_dispatcher::notifications::process_payload(
            &database.database,
            700_100_200,
            Some(42),
            wrong_type,
            1_800_000_001,
        )
        .await,
        ratatoskr_telegram_dispatcher::notifications::TransportDisposition::Term
    );

    let rows = sqlx::query(
        "select stream_sequence, event_id, failure_class, to_jsonb(failure) as stored
         from telegram.notification_transport_failures failure order by stream_sequence",
    )
    .fetch_all(database.pool())
    .await
    .expect("failure evidence");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<i64, _>("stream_sequence"), 41);
    assert_eq!(rows[0].get::<&str, _>("failure_class"), "invalid_envelope");
    assert_eq!(rows[1].get::<&str, _>("failure_class"), "wrong_event_type");
    for row in rows {
        let stored: serde_json::Value = row.get("stored");
        let rendered = stored.to_string();
        for private in ["body secret", "private", "payload", "title", "chat_id"] {
            assert!(!rendered.contains(private), "content leaked: {rendered}");
        }
    }

    let exposition = guard.metrics_handle().render();
    assert!(exposition.contains("telegram_notification_events_total"));
    for outcome in [
        "received",
        "duplicate",
        "enabled",
        "suppressed",
        "deferred",
        "enqueued",
        "delivered",
        "retry",
        "terminal",
    ] {
        assert!(
            exposition.contains(&format!("outcome=\"{outcome}\"")),
            "missing {outcome}: {exposition}"
        );
    }
    assert!(exposition.contains("class=\"other\""));
    assert!(exposition.contains("telegram_notification_backlog 7"));
    assert!(exposition.contains("telegram_notification_lag 8"));
    assert!(!exposition.contains("carrier_pigeon"));
    assert!(!exposition.contains("body secret"));
    database.cleanup().await.expect("cleanup");
}

fn notification_envelope(event_id: &str, notification_id: &str, class: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "event_id": event_id,
        "event_type": "platform.notification.raised.v1",
        "occurred_at": "2026-08-25T12:00:00Z",
        "producer": "ratatoskr-platform",
        "aggregate_id": format!("notification:{notification_id}"),
        "correlation_id": "operation:018f0000-0000-7000-8000-000000000302",
        "tenant_id": "user:018f0000-0000-7000-8000-000000000005",
        "schema_version": 1,
        "payload": {
            "notification_id": notification_id,
            "class_registry_version": 1,
            "class": class,
            "recipient": "user:018f0000-0000-7000-8000-000000000005",
            "title": "Safe synthetic notification"
        }
    }))
    .expect("notification envelope")
}
