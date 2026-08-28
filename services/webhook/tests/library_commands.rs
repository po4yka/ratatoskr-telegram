//! Search/read-state commands over the real acknowledged intake worker and a fake Platform.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use ratatoskr_telegram_webhook::intake::{self, CaptureContext, Intake, IntakeSettings};
use secrecy::SecretString;
use serde_json::json;
use sqlx::Row as _;
use telegram_persistence::IdentityProfile;
use telegram_persistence::test_support::TestDatabase;
use tower::ServiceExt as _;

const BOT_ID: i64 = 700_100_200;
const OWNER: i64 = 900_700_601;
const SECRET: &str = "webhook-secret-0123456789abcdef";
const AUDIENCE: &str = "ratatoskr-edge-test";
const INTERNAL_USER_ID: &str = "018f0000-0000-7000-8000-00000000abcd";
const SEED: [u8; 32] = [
    7, 6, 5, 4, 3, 2, 1, 0, 7, 6, 5, 4, 3, 2, 1, 0, 7, 6, 5, 4, 3, 2, 1, 0, 7, 6, 5, 4, 3, 2, 1, 0,
];

#[derive(Default)]
struct PlatformState {
    exchange_calls: AtomicU64,
    searches: std::sync::Mutex<Vec<BTreeMap<String, String>>>,
    capabilities: Vec<String>,
    library_items: std::sync::Mutex<Vec<serde_json::Value>>,
    read_calls: AtomicU64,
    read_behavior: std::sync::atomic::AtomicU8,
    search_behavior: std::sync::atomic::AtomicU8,
}

const READ_NOT_FOUND: u8 = 1;
const READ_UNAVAILABLE: u8 = 2;
const READ_TIMEOUT: u8 = 3;
const SEARCH_TIMEOUT: u8 = 1;

async fn platform_harness(capabilities: &[&str]) -> (url::Url, Arc<PlatformState>) {
    let state = Arc::new(PlatformState {
        capabilities: capabilities.iter().map(ToString::to_string).collect(),
        ..PlatformState::default()
    });
    let app =
        Router::new()
            .route(
                "/v1/sessions/telegram",
                post(|State(state): State<Arc<PlatformState>>| async move {
                    state.exchange_calls.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::CREATED,
                        Json(json!({
                            "credential": "synthetic-session-credential",
                            "expires_at": "2030-01-01T00:00:00Z",
                            "user_id": INTERNAL_USER_ID
                        })),
                    )
                }),
            )
            .route(
                "/v1/capabilities",
                get(|State(state): State<Arc<PlatformState>>| async move {
                    Json(json!({
                        "api_version": "1.0",
                        "minimum_client_versions": {"web":"1.0", "mobile":"1.0"},
                        "capabilities": state.capabilities,
                        "services": []
                    }))
                }),
            )
            .route(
                "/v1/library/search",
                get(
                    |State(state): State<Arc<PlatformState>>, uri: http::Uri| async move {
                        if state.search_behavior.load(Ordering::SeqCst) == SEARCH_TIMEOUT {
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                        let query =
                            url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
                                .into_owned()
                                .collect();
                        state.searches.lock().expect("search lock").push(query);
                        let items = state.library_items.lock().expect("items lock").clone();
                        Json(json!({
                            "items": items,
                            "limit": 5,
                            "offset": 0,
                            "has_more": false
                        }))
                    },
                ),
            )
            .route(
                "/v1/library/items/{analysis_id}/read-state",
                put(
                    |State(state): State<Arc<PlatformState>>,
                     Path(_analysis_id): Path<uuid::Uuid>| async move {
                        state.read_calls.fetch_add(1, Ordering::SeqCst);
                        match state.read_behavior.load(Ordering::SeqCst) {
                            READ_NOT_FOUND => {
                                (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"})))
                            }
                            READ_UNAVAILABLE => (
                                StatusCode::SERVICE_UNAVAILABLE,
                                Json(json!({"error": "unavailable"})),
                            ),
                            READ_TIMEOUT => {
                                tokio::time::sleep(Duration::from_millis(250)).await;
                                (StatusCode::OK, Json(json!({"read_state": "read"})))
                            }
                            _ => (StatusCode::OK, Json(json!({"read_state": "read"}))),
                        }
                    },
                ),
            )
            .with_state(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Platform harness");
    let address = listener.local_addr().expect("Platform harness address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).into_future().await;
    });
    (
        url::Url::parse(&format!("http://{address}")).expect("Platform URL"),
        state,
    )
}

fn library_item(index: u128, title: &str, snippet: &str, read_state: &str) -> serde_json::Value {
    json!({
        "analysis_id": uuid::Uuid::from_u128(index),
        "document_id": uuid::Uuid::from_u128(index + 100),
        "title": title,
        "snippet": snippet,
        "score": 1.0,
        "read_state": read_state
    })
}

fn hostile_library_items() -> Vec<serde_json::Value> {
    [
        (1, "one", 'A', "unread"),
        (2, "two", 'B', "unread"),
        (3, "three", 'C', "read"),
        (4, "four", 'D', "unread"),
        (5, "five", 'E', "unread"),
    ]
    .into_iter()
    .map(|(index, label, padding, state)| {
        library_item(
            index,
            &format!("<{label}>&{}", padding.to_string().repeat(400)),
            &format!("<script>{label}</script>").repeat(40),
            state,
        )
    })
    .chain(std::iter::once(library_item(
        6,
        "must-not-render",
        "six",
        "unread",
    )))
    .collect()
}

struct Fixture {
    database: TestDatabase,
    app: Router,
    _blob_root: tempfile::TempDir,
}

impl Fixture {
    async fn create(platform_url: &url::Url) -> Self {
        Self::create_with_timeout(platform_url, Duration::from_secs(3)).await
    }

    async fn create_with_timeout(platform_url: &url::Url, timeout: Duration) -> Self {
        let database = TestDatabase::create().await.expect("disposable database");
        database
            .database
            .ensure_identity(OWNER, &IdentityProfile::default())
            .await
            .expect("enabled owner identity");
        let client = platform_api::Client::new(platform_url, timeout).expect("Platform client");
        let issuer = platform_api::assertion::AssertionIssuer::from_seed(&SEED, AUDIENCE)
            .expect("assertion issuer");
        let sessions = Arc::new(platform_api::session::SessionSource::new(
            client,
            issuer,
            Box::new(platform_api::session::SystemClock),
        ));
        let bot_api = bot_api::Client::new(
            &SecretString::new("synthetic-bot-token".into()),
            platform_url,
            Duration::from_secs(3),
        )
        .expect("Bot API client");
        let blob_root = tempfile::tempdir().expect("blob root");
        let blobs =
            ratatoskr_telegram_blob_store::BlobStore::open(blob_root.path()).expect("blob store");
        let settings = IntakeSettings {
            secret: SecretString::new(SECRET.into()),
            max_body_bytes: 4096,
            bot_id: BOT_ID,
            queue_capacity: 32,
        };
        let (intake, receiver) = Intake::new(settings, database.database.clone());
        let context = CaptureContext::new(sessions, bot_api, blobs, 1024);
        tokio::spawn(intake::run_worker(
            intake.database.clone(),
            receiver,
            Some(context),
        ));
        Self {
            database,
            app: intake.router(),
            _blob_root: blob_root,
        }
    }

    async fn issue_read_token(&self, analysis_id: uuid::Uuid) -> String {
        let now = current_unix_time();
        self.database
            .database
            .issue_library_read_intent(
                telegram_persistence::interaction_tokens::NewLibraryReadIntent {
                    scope: telegram_persistence::interaction_tokens::LibraryReadScope {
                        bot_id: BOT_ID,
                        telegram_user_id: OWNER,
                        internal_user_id: INTERNAL_USER_ID.parse().expect("internal user id"),
                        chat_id: OWNER,
                    },
                    analysis_id,
                    expires_at: now + 900,
                },
                now,
            )
            .await
            .expect("read token")
    }

    async fn replies(&self) -> Vec<String> {
        sqlx::query_scalar(
            "select payload->>'text' from telegram.outbound_jobs order by created_at, id",
        )
        .fetch_all(self.database.pool())
        .await
        .expect("queued replies")
    }

    async fn deliver(&self, update_id: i64, text: &str) {
        let update = json!({
            "update_id": update_id,
            "message": {
                "message_id": update_id,
                "from": {"id": OWNER, "is_bot": false, "first_name": "Owner"},
                "date": 1_760_000_000_i64,
                "chat": {"id": OWNER, "type": "private", "first_name": "Owner"},
                "text": text
            }
        });
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook")
                    .header("content-type", "application/json")
                    .header("x-telegram-bot-api-secret-token", SECRET)
                    .body(axum::body::Body::from(update.to_string()))
                    .expect("webhook request"),
            )
            .await
            .expect("webhook response");
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.into_body().collect().await.expect("ack body");
    }

    async fn settled(&self, update_id: i64) -> String {
        for _ in 0..200 {
            if let Ok(row) = sqlx::query(
                "select state from telegram.updates where bot_id = $1 and update_id = $2",
            )
            .bind(BOT_ID)
            .bind(update_id)
            .fetch_one(self.database.pool())
            .await
            {
                let state: String = row.get("state");
                if !matches!(state.as_str(), "accepted" | "processing") {
                    return state;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("update {update_id} did not settle")
    }
}

fn current_unix_time() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_secs(),
    )
    .expect("current Unix time")
}

#[derive(Clone)]
struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CapturedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log capture lock").write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn telemetry_observers() -> (
    &'static Arc<Mutex<Vec<u8>>>,
    &'static metrics_exporter_prometheus::PrometheusHandle,
) {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    static METRICS: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
    let logs = LOGS.get_or_init(|| {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&logs);
        tracing_subscriber::fmt()
            .with_writer(move || CapturedWriter(Arc::clone(&writer)))
            .with_ansi(false)
            .try_init()
            .expect("test tracing subscriber");
        logs
    });
    let metrics = METRICS.get_or_init(|| {
        metrics_exporter_prometheus::PrometheusBuilder::new()
            .install_recorder()
            .expect("test metrics recorder")
    });
    (logs, metrics)
}

/// Search and unread delegate exact bounded queries only from the post-ack worker.
#[tokio::test]
async fn search_and_unread_map_to_bounded_platform_queries_after_ack() {
    let (platform_url, state) = platform_harness(&["library.read_state", "library.search"]).await;
    let fixture = Fixture::create(&platform_url).await;

    fixture.deliver(1001, "/search recovery").await;
    fixture.deliver(1002, "/unread").await;

    assert_eq!(fixture.settled(1001).await, "processed");
    assert_eq!(fixture.settled(1002).await, "processed");
    let searches = state.searches.lock().expect("search lock");
    assert_eq!(searches.len(), 2);
    assert_eq!(
        searches[0],
        BTreeMap::from([
            ("limit".to_owned(), "5".to_owned()),
            ("offset".to_owned(), "0".to_owned()),
            ("q".to_owned(), "recovery".to_owned()),
        ])
    );
    assert_eq!(
        searches[1],
        BTreeMap::from([
            ("limit".to_owned(), "5".to_owned()),
            ("offset".to_owned(), "0".to_owned()),
            ("q".to_owned(), String::new()),
            ("read_state".to_owned(), "unread".to_owned()),
        ])
    );
    assert_eq!(state.exchange_calls.load(Ordering::SeqCst), 1);
}

/// Invalid command shapes and a missing capability are claimed locally without domain calls.
#[tokio::test]
async fn invalid_library_forms_and_absent_capabilities_never_query_platform() {
    let (platform_url, state) = platform_harness(&[]).await;
    let fixture = Fixture::create(&platform_url).await;
    let commands = [
        "/search".to_owned(),
        format!("/search {}", "я".repeat(257)),
        "/unread extra".to_owned(),
        "/read malformed".to_owned(),
        "/search recovery".to_owned(),
    ];

    for (index, command) in commands.iter().enumerate() {
        let update_id = 2001 + i64::try_from(index).expect("small test index");
        fixture.deliver(update_id, command).await;
        assert_eq!(fixture.settled(update_id).await, "processed");
    }

    assert!(state.searches.lock().expect("search lock").is_empty());
    let replies: Vec<String> = sqlx::query_scalar(
        "select payload->>'text' from telegram.outbound_jobs order by created_at, id",
    )
    .fetch_all(fixture.database.pool())
    .await
    .expect("queued replies");
    assert_eq!(replies.len(), 5);
    assert!(replies[..4].iter().all(|reply| reply.starts_with("Usage:")));
    assert_eq!(replies[4], "Library search is temporarily unavailable.");
}

/// Rendering is one bounded HTML message and unread mutations are reachable only through scoped tokens.
#[tokio::test]
async fn result_render_is_escaped_bounded_and_issues_only_owner_scoped_read_tokens() {
    let (platform_url, state) = platform_harness(&["library.read_state", "library.search"]).await;
    *state.library_items.lock().expect("items lock") = hostile_library_items();
    let fixture = Fixture::create(&platform_url).await;

    fixture.deliver(3001, "/unread").await;
    assert_eq!(fixture.settled(3001).await, "processed");

    let replies: Vec<String> = sqlx::query_scalar(
        "select payload->>'text' from telegram.outbound_jobs order by created_at, id",
    )
    .fetch_all(fixture.database.pool())
    .await
    .expect("rendered reply");
    assert_eq!(replies.len(), 1);
    let reply = &replies[0];
    assert!(reply.chars().count() < 4096);
    assert!(reply.contains("&lt;one&gt;&amp;"));
    assert!(!reply.contains("<script>"));
    assert!(!reply.contains("must-not-render"));

    assert_rendered_tokens_are_owner_scoped(&fixture, reply).await;
    assert_search_without_read_capability_has_no_tokens().await;
}

async fn assert_rendered_tokens_are_owner_scoped(fixture: &Fixture, reply: &str) {
    let tokens: Vec<_> = reply
        .split("/read ")
        .skip(1)
        .filter_map(|suffix| suffix.get(..64))
        .collect();
    assert_eq!(
        tokens.len(),
        4,
        "only the four rendered unread items get actions"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from telegram.interaction_tokens where action = 'library_read'",
        )
        .fetch_one(fixture.database.pool())
        .await
        .expect("read token count"),
        4,
    );
    for token in tokens {
        assert_eq!(token.len(), 64);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
        let now = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_secs(),
        )
        .expect("current Unix time");
        let owner_scope = telegram_persistence::interaction_tokens::LibraryReadScope {
            bot_id: BOT_ID,
            telegram_user_id: OWNER,
            internal_user_id: INTERNAL_USER_ID.parse().expect("internal user id"),
            chat_id: OWNER,
        };
        let mut foreign_scope = owner_scope;
        foreign_scope.internal_user_id = uuid::Uuid::now_v7();
        assert_eq!(
            fixture
                .database
                .database
                .resolve_library_read_intent(
                    telegram_persistence::interaction_tokens::LibraryReadPresentation {
                        token,
                        scope: foreign_scope,
                        now,
                    },
                )
                .await
                .expect("foreign resolution"),
            Err(telegram_persistence::interaction_tokens::TokenRefusal::ScopeMismatch),
        );
        assert!(
            fixture
                .database
                .database
                .resolve_library_read_intent(
                    telegram_persistence::interaction_tokens::LibraryReadPresentation {
                        token,
                        scope: owner_scope,
                        now,
                    },
                )
                .await
                .expect("owner resolution")
                .is_ok(),
        );
    }
}

async fn assert_search_without_read_capability_has_no_tokens() {
    let (search_only_url, search_only_state) = platform_harness(&["library.search"]).await;
    *search_only_state.library_items.lock().expect("items lock") =
        vec![library_item(7, "<search-only>", "visible", "unread")];
    let search_only = Fixture::create(&search_only_url).await;
    search_only.deliver(3002, "/search visible").await;
    assert_eq!(search_only.settled(3002).await, "processed");
    let search_only_replies = search_only.replies().await;
    let search_only_reply = search_only_replies.first().expect("search-only reply");
    assert!(search_only_reply.contains("&lt;search-only&gt;"));
    assert!(!search_only_reply.contains("/read "));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from telegram.interaction_tokens where action = 'library_read'",
        )
        .fetch_one(search_only.database.pool())
        .await
        .expect("read token count"),
        0,
    );
}

/// A read token has one mutation winner and every response reflects authoritative knowledge.
#[tokio::test]
async fn read_command_has_one_winner_and_reports_success_not_found_unavailable_and_unknown_truthfully()
 {
    let capabilities = ["library.read_state", "library.search"];

    let (success_url, success_state) = platform_harness(&capabilities).await;
    let success = Fixture::create(&success_url).await;
    let success_token = success.issue_read_token(uuid::Uuid::from_u128(41)).await;
    success
        .deliver(4001, &format!("/read {success_token}"))
        .await;
    success
        .deliver(4002, &format!("/read {success_token}"))
        .await;
    assert_eq!(success.settled(4001).await, "processed");
    assert_eq!(success.settled(4002).await, "processed");
    assert_eq!(success_state.read_calls.load(Ordering::SeqCst), 1);
    let success_replies = success.replies().await;
    assert_eq!(success_replies.len(), 2);
    assert_eq!(
        success_replies
            .iter()
            .filter(|reply| reply.contains("marked as read"))
            .count(),
        1,
    );
    assert_eq!(
        success_replies
            .iter()
            .filter(|reply| reply.contains("expired"))
            .count(),
        1,
    );

    let (missing_url, missing_state) = platform_harness(&capabilities).await;
    missing_state
        .read_behavior
        .store(READ_NOT_FOUND, Ordering::SeqCst);
    let missing = Fixture::create(&missing_url).await;
    let missing_token = missing.issue_read_token(uuid::Uuid::from_u128(42)).await;
    missing
        .deliver(4101, &format!("/read {missing_token}"))
        .await;
    assert_eq!(missing.settled(4101).await, "processed");
    assert_eq!(missing_state.read_calls.load(Ordering::SeqCst), 1);
    assert!(
        missing
            .replies()
            .await
            .first()
            .expect("missing-item reply")
            .contains("no longer available")
    );

    let (unavailable_url, unavailable_state) = platform_harness(&capabilities).await;
    unavailable_state
        .read_behavior
        .store(READ_UNAVAILABLE, Ordering::SeqCst);
    let unavailable = Fixture::create(&unavailable_url).await;
    let unavailable_token = unavailable
        .issue_read_token(uuid::Uuid::from_u128(43))
        .await;
    unavailable
        .deliver(4201, &format!("/read {unavailable_token}"))
        .await;
    assert_eq!(unavailable.settled(4201).await, "processed");
    assert_eq!(unavailable_state.read_calls.load(Ordering::SeqCst), 2);
    assert!(
        unavailable
            .replies()
            .await
            .first()
            .expect("unavailable reply")
            .contains("temporarily unavailable")
    );

    let (timeout_url, timeout_state) = platform_harness(&capabilities).await;
    timeout_state
        .read_behavior
        .store(READ_TIMEOUT, Ordering::SeqCst);
    let timeout = Fixture::create_with_timeout(&timeout_url, Duration::from_millis(75)).await;
    let timeout_token = timeout.issue_read_token(uuid::Uuid::from_u128(44)).await;
    timeout
        .deliver(4301, &format!("/read {timeout_token}"))
        .await;
    assert_eq!(timeout.settled(4301).await, "processed");
    assert_eq!(timeout_state.read_calls.load(Ordering::SeqCst), 2);
    let timeout_replies = timeout.replies().await;
    let timeout_reply = timeout_replies.first().expect("timeout reply");
    assert!(timeout_reply.contains("unknown"));
    assert!(timeout_reply.contains("/unread"));
    assert!(!timeout_reply.contains("marked as read"));
}

/// Library telemetry records finite classes and never records private library inputs or identities.
#[tokio::test]
async fn library_telemetry_contains_only_command_and_outcome_classes() {
    let (logs, metrics) = telemetry_observers();
    logs.lock().expect("log capture lock").clear();
    let (platform_url, state) = platform_harness(&["library.search"]).await;
    state
        .search_behavior
        .store(SEARCH_TIMEOUT, Ordering::SeqCst);
    let fixture = Fixture::create_with_timeout(&platform_url, Duration::from_millis(75)).await;

    fixture.deliver(5001, "/search private phrase").await;
    assert_eq!(fixture.settled(5001).await, "processed");

    let log_output =
        String::from_utf8(logs.lock().expect("log capture lock").clone()).expect("UTF-8 logs");
    let metric_output = metrics.render();
    let combined = format!("{log_output}\n{metric_output}");
    assert!(metric_output.contains("telegram_library_commands_total"));
    assert!(combined.contains("search"));
    assert!(combined.contains("timeout"));
    for prohibited in [
        "private phrase",
        INTERNAL_USER_ID,
        &OWNER.to_string(),
        &BOT_ID.to_string(),
        "analysis_id",
        "document_id",
        "/read ",
    ] {
        assert!(
            !combined.contains(prohibited),
            "telemetry leaked {prohibited}"
        );
    }
}
