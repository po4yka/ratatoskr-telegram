//! Webhook admission, end to end over the real router stack: secret gate, limits, schema check,
//! deduplication, fast acknowledgment, and the async handoff.
//!
//! Each test drives the router through `tower::ServiceExt::oneshot` against its own disposable
//! database, holding the queue receiver so that processing cannot race the assertions. No test
//! contacts Telegram.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::body::Body;
use http::header::CONTENT_TYPE;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use ratatoskr_telegram_webhook::intake::{self, Intake, IntakeSettings, QueuedUpdate};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::Row;
use telegram_persistence::IdentityProfile;
use telegram_persistence::test_support::TestDatabase;
use tower::ServiceExt;

/// The synthetic bot identity every test uses.
const BOT_ID: i64 = 700_100_200;

/// The deployment owner the fixtures bootstrap, mirroring what startup does from configuration.
const OWNER_TELEGRAM_USER_ID: i64 = 900_700_601;

/// A synthetic high-entropy webhook secret.
const SECRET: &str = "webhook-secret-0123456789abcdef";

/// A valid Bot API message update from the owner in a private chat, with the given id.
fn message_update(update_id: i64) -> Value {
    message_from(
        update_id,
        OWNER_TELEGRAM_USER_ID,
        OWNER_TELEGRAM_USER_ID,
        "private",
    )
}

/// A message update from an arbitrary sender into an arbitrary chat.
fn message_from(update_id: i64, sender: i64, chat_id: i64, chat_type: &str) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": 55,
            "from": {"id": sender, "is_bot": false, "first_name": "Synthetic"},
            "date": 1_760_000_000_i64,
            "chat": {"id": chat_id, "type": chat_type, "first_name": "Synthetic"},
            "text": "https://example.test/article",
        },
    })
}

/// A message update delivered into a group conversation: the shape the policy must refuse
/// without creating a chat row for it.
fn group_message_update(update_id: i64, sender: i64) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": 56,
            "from": {"id": sender, "is_bot": false, "first_name": "Synthetic"},
            "date": 1_760_000_000_i64,
            "chat": {"id": -100_200_300, "type": "group", "title": "Synthetic Group"},
            "text": "https://example.test/article",
        },
    })
}

struct Fixture {
    database: TestDatabase,
    receiver: tokio::sync::mpsc::Receiver<QueuedUpdate>,
    app: axum::Router,
}

impl Fixture {
    /// A cap comfortably above real update sizes; limit tests build their own tighter fixture
    /// with [`Fixture::with_settings`].
    async fn create() -> Self {
        Self::with_settings(IntakeSettings {
            secret: SecretString::new(SECRET.into()),
            max_body_bytes: 4096,
            bot_id: BOT_ID,
            queue_capacity: 32,
        })
        .await
    }

    async fn with_settings(settings: IntakeSettings) -> Self {
        let database = TestDatabase::create().await.expect("disposable database");
        // The production bootstrap seeds exactly one enabled owner from configuration before any
        // delivery; every fixture does the same so its expectations are about the gate, not
        // about enrollment.
        database
            .database
            .ensure_identity(OWNER_TELEGRAM_USER_ID, &IdentityProfile::default())
            .await
            .expect("the fixture owner identity");
        let (intake, receiver) = Intake::new(settings, database.database.clone());
        let app = intake.router();
        Self {
            database,
            receiver,
            app,
        }
    }

    /// One admitted update, delivered as Telegram would: POST, JSON, correct secret.
    async fn deliver(&mut self, update: Value) -> StatusCode {
        self.send(
            Some(SECRET),
            Some("application/json"),
            update.to_string().into_bytes(),
        )
        .await
    }

    /// Raw control over every admission input.
    async fn send(
        &mut self,
        secret: Option<&str>,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> StatusCode {
        self.send_detailed(secret, content_type, body).await.0
    }

    /// As `send`, also returning the response body for the explicit-limit assertion.
    async fn send_detailed(
        &mut self,
        secret: Option<&str>,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> (StatusCode, Vec<u8>) {
        let mut builder = Request::builder().method("POST").uri("/webhook");
        if let Some(secret) = secret {
            builder = builder.header("x-telegram-bot-api-secret-token", secret);
        }
        if let Some(content_type) = content_type {
            builder = builder.header(CONTENT_TYPE, content_type);
        }
        // No automatic content-length: the streamed-read path is what an undeclared length tests.
        let request = builder.body(Body::from(body)).expect("request");
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("infallible for a router");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (status, bytes.to_vec())
    }

    async fn rows(&self) -> i64 {
        sqlx::query("select count(*)::bigint as n from telegram.updates")
            .fetch_one(self.database.pool())
            .await
            .expect("count")
            .get("n")
    }

    async fn states_of(&self, update_id: i64) -> Vec<String> {
        sqlx::query("select state from telegram.updates where update_id = $1")
            .bind(update_id)
            .fetch_all(self.database.pool())
            .await
            .expect("rows")
            .iter()
            .map(|row| row.get::<&str, _>("state").to_owned())
            .collect()
    }

    async fn cleanup(self) {
        self.database.cleanup().await.expect("cleanup");
    }
}

/// Missing secret header: 401 before anything is read or parsed, nothing written, nothing queued.
#[tokio::test]
async fn a_request_without_the_secret_header_is_unauthorized() {
    let mut fixture = Fixture::create().await;
    let status = fixture
        .send(
            None,
            Some("application/json"),
            message_update(1).to_string().into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(fixture.rows().await, 0);
    assert!(fixture.receiver.try_recv().is_err());
    fixture.cleanup().await;
}

/// A forged secret: same outcome. This is the case the constant-time comparison exists for.
#[tokio::test]
async fn a_forged_secret_is_unauthorized() {
    let mut fixture = Fixture::create().await;
    let status = fixture
        .send(
            Some("webhook-secret-ffffffffffffffff"),
            Some("application/json"),
            message_update(2).to_string().into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(fixture.rows().await, 0);
    assert!(fixture.receiver.try_recv().is_err());
    fixture.cleanup().await;
}

/// Method restriction: a GET that presents the CORRECT secret still answers 405.
#[tokio::test]
async fn a_non_post_method_is_refused() {
    let fixture = Fixture::create().await;
    let request = Request::builder()
        .method("GET")
        .uri("/webhook")
        .header("x-telegram-bot-api-secret-token", SECRET)
        .body(Body::empty())
        .expect("request");
    let response = fixture
        .app
        .clone()
        .oneshot(request)
        .await
        .expect("infallible for a router");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    fixture.cleanup().await;
}

/// Content-type restriction.
#[tokio::test]
async fn a_non_json_content_type_is_refused() {
    let mut fixture = Fixture::create().await;
    let status = fixture
        .send(Some(SECRET), Some("text/plain"), b"{}".to_vec())
        .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(fixture.rows().await, 0);
    fixture.cleanup().await;
}

/// Declared size above the cap: 413 with an explicit limit response, before the body is read.
#[tokio::test]
async fn an_oversized_declared_body_gets_the_explicit_limit_response() {
    let mut fixture = Fixture::with_settings(IntakeSettings {
        secret: SecretString::new(SECRET.into()),
        max_body_bytes: 64,
        bot_id: BOT_ID,
        queue_capacity: 32,
    })
    .await;
    let (status, body) = fixture
        .send_detailed(Some(SECRET), Some("application/json"), vec![b'x'; 100])
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        String::from_utf8_lossy(&body).contains("limit"),
        "the response must name the limit: {}",
        String::from_utf8_lossy(&body),
    );
    assert_eq!(fixture.rows().await, 0);
    assert!(fixture.receiver.try_recv().is_err());
    fixture.cleanup().await;
}

/// An undeclared (streamed) body cannot exceed the cap either: admission checks media type first,
/// then cuts the capped read off mid-stream.
#[tokio::test]
async fn a_streamed_body_cannot_exceed_the_cap() {
    let mut fixture = Fixture::with_settings(IntakeSettings {
        secret: SecretString::new(SECRET.into()),
        max_body_bytes: 64,
        bot_id: BOT_ID,
        queue_capacity: 32,
    })
    .await;
    // No content-length header: the cap has to fire while READING, not from the declaration.
    let (status, _) = fixture
        .send_detailed(Some(SECRET), Some("application/json"), vec![b'y'; 100])
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(fixture.rows().await, 0);
    fixture.cleanup().await;
}

/// Malformed JSON: acknowledged 200 so Telegram stops retrying, logged, never recorded.
#[tokio::test]
async fn a_malformed_payload_is_acked_without_being_recorded() {
    let mut fixture = Fixture::create().await;
    let status = fixture
        .send(
            Some(SECRET),
            Some("application/json"),
            b"{not json".to_vec(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fixture.rows().await, 0);
    assert!(fixture.receiver.try_recv().is_err());
    fixture.cleanup().await;
}

/// Valid envelope, unknown kind: accepted and recorded as unsupported, not treated as malformed.
#[tokio::test]
async fn an_unsupported_kind_is_accepted_and_recorded() {
    let mut fixture = Fixture::create().await;
    let update = json!({"update_id": 3001, "origin_message": {"unknown_to_this_build": true}});
    assert_eq!(fixture.deliver(update).await, StatusCode::OK);
    assert_eq!(fixture.rows().await, 1);
    assert_eq!(
        sqlx::query("select kind from telegram.updates where update_id = 3001")
            .fetch_one(fixture.database.pool())
            .await
            .expect("the row")
            .get::<&str, _>("kind"),
        "unsupported",
    );
    // The queued item exists for the worker; its settlement to `unsupported` is covered by
    // `the_worker_settles_every_kind_it_classifies`.
    drop(fixture.receiver.try_recv().expect("queued"));
    fixture.cleanup().await;
}

/// The happy path: one delivery, one row, exactly one queued handoff.
#[tokio::test]
async fn a_valid_update_is_accepted_once_and_queued_once() {
    let mut fixture = Fixture::create().await;
    assert_eq!(fixture.deliver(message_update(4001)).await, StatusCode::OK);
    assert_eq!(fixture.rows().await, 1);
    let item = fixture.receiver.try_recv().expect("queued");
    assert_eq!(item.update.id.0, 4001);
    assert!(
        fixture.receiver.try_recv().is_err(),
        "nothing else was queued"
    );

    // The worker settles it processed.
    intake::process_one(&fixture.database.database, &item, None).await;
    assert_eq!(fixture.states_of(4001).await, ["processed"]);
    fixture.cleanup().await;
}

/// A replacement worker recovers an acknowledged update without the process-local notification.
#[tokio::test]
async fn an_admitted_update_is_processed_after_worker_restart() {
    let mut fixture = Fixture::create().await;
    assert_eq!(fixture.deliver(message_update(4002)).await, StatusCode::OK);

    let Fixture {
        database,
        receiver,
        app,
    } = fixture;
    drop(receiver);
    drop(app);

    let (restart_sender, restart_receiver) = tokio::sync::mpsc::channel(1);
    drop(restart_sender);
    intake::run_worker(database.database.clone(), restart_receiver, None).await;

    let state: String = sqlx::query("select state from telegram.updates where update_id = 4002")
        .fetch_one(database.pool())
        .await
        .expect("the admitted update remains durable")
        .get("state");
    database.cleanup().await.expect("cleanup");
    assert_eq!(state, "processed");
}

/// Duplicate delivery has no effect the second time — including older ids arriving out of order —
/// while genuinely unseen older ids still process.
#[tokio::test]
async fn duplicates_are_dropped_once_ever_across_out_of_order_deliveries() {
    let mut fixture = Fixture::create().await;
    assert_eq!(fixture.deliver(message_update(5100)).await, StatusCode::OK);
    assert_eq!(fixture.deliver(message_update(5042)).await, StatusCode::OK);
    // Redelivery of 42 after 100: exact-match dedupe drops it regardless of ordering.
    assert_eq!(fixture.deliver(message_update(5042)).await, StatusCode::OK);
    // An unseen id BELOW the highest seen id is not a duplicate: it never arrived before.
    assert_eq!(fixture.deliver(message_update(5099)).await, StatusCode::OK);

    let mut seen = Vec::new();
    while let Ok(item) = fixture.receiver.try_recv() {
        seen.push(item.update.id.0);
    }
    assert_eq!(seen, [5100, 5042, 5099]);
    assert_eq!(fixture.rows().await, 3);
    fixture.cleanup().await;
}

/// The acknowledgment-latency contract: requests complete promptly while processing is stalled,
/// and every queued item processes once the worker resumes.
#[tokio::test]
async fn acknowledgments_complete_while_processing_is_stalled() {
    let mut fixture = Fixture::create().await;
    let bodies: Vec<Value> = (6000..6006).map(message_update).collect();

    let started = Instant::now();
    for update in &bodies {
        let status = fixture.deliver(update.clone()).await;
        assert_eq!(status, StatusCode::OK);
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "six admissions took {elapsed:?} with the worker fully blocked",
    );

    // Nothing consumed the queue during those deliveries.
    let mut items = Vec::new();
    while let Ok(item) = fixture.receiver.try_recv() {
        items.push(item);
    }
    assert_eq!(items.len(), bodies.len());

    // Resume "processing": every accepted update settles.
    let database = fixture.database.database.clone();
    for item in &items {
        intake::process_one(&database, item, None).await;
    }
    for update_id in 6000..6006 {
        assert_eq!(fixture.states_of(update_id).await, ["processed"]);
    }
    fixture.cleanup().await;
}

/// Queue saturation refuses 503 with NO side effect, so Telegram's retry can succeed later.
#[tokio::test]
async fn a_saturated_queue_refuses_without_persisting() {
    let mut fixture = Fixture::with_settings(IntakeSettings {
        secret: SecretString::new(SECRET.into()),
        max_body_bytes: 4096,
        bot_id: BOT_ID,
        queue_capacity: 1,
    })
    .await;

    assert_eq!(fixture.deliver(message_update(7001)).await, StatusCode::OK);
    assert_eq!(
        fixture.deliver(message_update(7002)).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        fixture.rows().await,
        1,
        "the refused update must leave no row"
    );
    fixture.cleanup().await;
}

/// Storage failure refuses 503: no acknowledgment of success was given, Telegram retries.
#[tokio::test]
async fn a_storage_failure_refuses_with_retryable_overload() {
    let mut fixture = Fixture::create().await;
    fixture.database.database.close().await;
    let status = fixture
        .send(
            Some(SECRET),
            Some("application/json"),
            message_update(8001).to_string().into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    fixture.cleanup().await;
}

/// End-to-end worker behaviour: recognized kinds settle processed, unrecognized unsupported.
#[tokio::test]
async fn the_worker_settles_every_kind_it_classifies() {
    let mut fixture = Fixture::create().await;
    let callback = json!({
        "update_id": 9001,
        "callback_query": {
            "id": "cq-test", "chat_instance": "-1",
            "from": {"id": 900_700_601, "is_bot": false, "first_name": "Synthetic"},
            "message": {
                "message_id": 77,
                "date": 1_760_000_000_i64,
                "chat": {"id": 900_700_601, "type": "private", "first_name": "Synthetic"},
            },
            "data": "opaque-intent-token",
        },
    });
    let unknown = json!({"update_id": 9002, "origin_message": {}});
    for update in [message_update(9003), callback, unknown] {
        assert_eq!(fixture.deliver(update).await, StatusCode::OK);
    }

    let database = fixture.database.database.clone();
    let receiver = std::mem::replace(&mut fixture.receiver, tokio::sync::mpsc::channel(1).1);
    let worker = tokio::spawn(intake::run_worker(database, receiver, None));
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let pending: i64 = sqlx::query(
            "select count(*)::bigint as n from telegram.updates where state <> 'processed'
             and state <> 'unsupported'",
        )
        .fetch_one(fixture.database.pool())
        .await
        .expect("count")
        .get("n");
        if pending == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    worker.abort();
    assert_eq!(fixture.states_of(9003).await, ["processed"]);
    assert_eq!(fixture.states_of(9001).await, ["processed"]);
    assert_eq!(fixture.states_of(9002).await, ["unsupported"]);
    fixture.cleanup().await;
}

/// An update from a sender with no identity record never reaches domain processing: it settles
/// `denied`, its processable payload is minimized, nothing enrolls the stranger, and no chat row
/// appears. There is no outbound Bot API call to observe: the worker holds no client at all, and
/// the boot suite pins the process-wide total (exactly one startup `getMe`) so any future reply
/// path would break these assertions together.
#[tokio::test]
async fn unauthorized_sender_settles_denied_without_outbound_calls() {
    const STRANGER: i64 = 800_800_801;

    let mut fixture = Fixture::create().await;
    assert_eq!(
        fixture
            .deliver(message_from(12_001, STRANGER, STRANGER, "private"))
            .await,
        StatusCode::OK
    );
    let item = fixture.receiver.try_recv().expect("queued");
    intake::process_one(&fixture.database.database, &item, None).await;

    assert_eq!(fixture.states_of(12_001).await, ["denied"]);
    let minimized: bool = sqlx::query(
        "select payload is null as minimized from telegram.updates where update_id = 12001",
    )
    .fetch_one(fixture.database.pool())
    .await
    .expect("the settled row")
    .get("minimized");
    assert!(minimized, "a denied update must not keep its payload");

    let strangers: i64 = sqlx::query(
        "select count(*)::bigint as n from telegram.identities where telegram_user_id = $1",
    )
    .bind(STRANGER)
    .fetch_one(fixture.database.pool())
    .await
    .expect("count")
    .get("n");
    assert_eq!(strangers, 0, "an unauthorized sender must not be enrolled");

    let chats: i64 =
        sqlx::query("select count(*)::bigint as n from telegram.chats where chat_id = $1")
            .bind(STRANGER)
            .fetch_one(fixture.database.pool())
            .await
            .expect("count")
            .get("n");
    assert_eq!(chats, 0, "a denied delivery must not create a chat row");

    fixture.cleanup().await;
}

/// The rest of the policy matrix: an enrolled-but-disabled identity and a group conversation
/// deny exactly like an unknown sender — same terminal state, same minimized payload — and a
/// group chat gains no row even when the sender is the enabled owner.
#[tokio::test]
async fn a_disabled_identity_and_a_group_chat_deny_like_an_unknown_sender() {
    const DISABLED_STRANGER: i64 = 800_800_802;

    let mut fixture = Fixture::create().await;

    // Enrolled yesterday, disabled since: the gate must not resurrect or admit them.
    fixture
        .database
        .database
        .ensure_identity(DISABLED_STRANGER, &IdentityProfile::default())
        .await
        .expect("the enrolled identity");
    sqlx::query(
        "update telegram.identities set access_state = 'disabled' where telegram_user_id = $1",
    )
    .bind(DISABLED_STRANGER)
    .execute(fixture.database.pool())
    .await
    .expect("the disable");

    for (update_id, update) in [
        (
            12_101,
            message_from(12_101, DISABLED_STRANGER, DISABLED_STRANGER, "private"),
        ),
        (12_201, group_message_update(12_201, OWNER_TELEGRAM_USER_ID)),
    ] {
        assert_eq!(fixture.deliver(update).await, StatusCode::OK);
        let item = fixture.receiver.try_recv().expect("queued");
        assert_eq!(item.update.id.0, u32::try_from(update_id).expect("fits"));
        intake::process_one(&fixture.database.database, &item, None).await;
        assert_eq!(fixture.states_of(update_id).await, ["denied"]);
        let minimized: bool = sqlx::query(
            "select payload is null as minimized from telegram.updates where update_id = $1",
        )
        .bind(update_id)
        .fetch_one(fixture.database.pool())
        .await
        .expect("the settled row")
        .get("minimized");
        assert!(minimized, "{update_id}: every denial minimizes the payload");
    }

    // Identical observable shape across classes: state, kind, no payload.
    let shapes: Vec<(String, String)> = sqlx::query(
        "select kind, coalesce(state, '') as state from telegram.updates where payload is null \
         and state in ('denied') order by update_id",
    )
    .fetch_all(fixture.database.pool())
    .await
    .expect("rows")
    .into_iter()
    .map(|row| {
        (
            row.get::<&str, _>("kind").to_owned(),
            row.get::<&str, _>("state").to_owned(),
        )
    })
    .collect();
    assert_eq!(shapes, vec![("message".to_owned(), "denied".to_owned()); 2]);

    let group_rows: i64 =
        sqlx::query("select count(*)::bigint as n from telegram.chats where chat_id = -100200300")
            .fetch_one(fixture.database.pool())
            .await
            .expect("count")
            .get("n");
    assert_eq!(group_rows, 0, "a refused group must gain no chat row");

    fixture.cleanup().await;
}

/// The bounded metric vocabulary appears in the exposition, with no request-controlled labels.
#[test]
fn outcomes_are_countable_without_content() {
    static GUARD: OnceLock<telegram_telemetry::TelemetryGuard> = OnceLock::new();
    let guard = GUARD.get_or_init(|| {
        telegram_telemetry::init(
            &telegram_core::config::TelemetryConfig::default(),
            telegram_core::RuntimeRole::Webhook,
        )
        .expect("the registry installs once per process")
    });

    let runtime = tokio::runtime::Runtime::new().expect("metrics runtime");
    runtime.block_on(async {
        let mut fixture = Fixture::create().await;
        fixture.deliver(message_update(11001)).await;
        fixture
            .send(None, Some("application/json"), b"{}".to_vec())
            .await; // unauthorized
        fixture
            .send(Some(SECRET), Some("text/plain"), b"{}".to_vec())
            .await; // wrong media type
        fixture.cleanup().await;
    });

    let exposition = guard.metrics_handle().render();
    for series in [
        "telegram_webhook_requests_total{outcome=\"accepted\"}",
        "telegram_webhook_requests_total{outcome=\"unauthorized\"}",
        "telegram_webhook_requests_total{outcome=\"wrong_media_type\"}",
        "telegram_updates_received_total{update_kind=\"message\"}",
        "telegram_webhook_duration_seconds",
    ] {
        assert!(
            exposition.contains(series),
            "{series} missing from:\n{exposition}"
        );
    }
}
