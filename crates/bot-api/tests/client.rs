//! The typed client, exercised only against a local harness server.
//!
//! Every test points `Client` at an in-process axum server that records what it receives and
//! answers with recorded fixtures. Nothing here contacts api.telegram.org.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::Path;
use axum::http::HeaderMap;
use axum::http::header;
use axum::response::IntoResponse as _;
use axum::response::Json;
use axum::routing::any;
use bot_api::{
    BotApiError, ChatAction, ChatId, Client, DownloadStream, File, MessageId, MessageOptions,
};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Value, json};
use url::Url;

/// The synthetic token every test uses. It is never a real credential.
const TOKEN: &str = "123456:TEST-harness-token";

/// One request the harness captured.
#[derive(Debug, Clone)]
struct Captured {
    /// The request path, e.g. `/bot<token>/GetMe`.
    path: String,
    /// The JSON body, when the method carries one.
    body: Option<Value>,
    /// The raw body bytes and the content type that framed them.
    raw: (String, axum::body::Bytes),
}

impl Captured {
    /// The value of one form field of a multipart body — `setWebhook` is always multipart in
    /// teloxide because of its optional certificate field, so its payload arrives as parts, not
    /// JSON. A minimal split: boundary from the content type, then name → value per part.
    fn form_field(&self, name: &str) -> Option<String> {
        let (content_type, bytes) = &self.raw;
        let boundary = content_type.split("boundary=").nth(1)?;
        let delimiter = format!("--{}", boundary.trim());
        let text = String::from_utf8_lossy(bytes);
        text.split(&delimiter).find_map(|part| {
            let (_, after_headers) = part.split_once("\r\n\r\n")?;
            let value = after_headers.strip_suffix("\r\n").unwrap_or(after_headers);
            let named = part.contains(&format!("name=\"{name}\""))
                || part.contains(&format!("name={name}"));
            named.then(|| value.to_owned())
        })
    }
}

/// A local Bot API server: answers every call with `respond`, records everything.
struct Harness {
    base_url: Url,
    captured: Arc<Mutex<Vec<Captured>>>,
}

impl Harness {
    async fn spawn(respond: impl Fn(&Captured) -> Value + Send + Sync + 'static) -> Self {
        Self::spawn_raw(move |captured| Json(respond(captured)).into_response()).await
    }

    /// Like [`Harness::spawn`], but the responder owns the whole response - raw byte bodies for
    /// the file-download routes that never carry JSON.
    async fn spawn_raw(
        respond: impl Fn(&Captured) -> axum::response::Response + Send + Sync + 'static,
    ) -> Self {
        let captured: Arc<Mutex<Vec<Captured>>> = Arc::default();
        let state = Arc::clone(&captured);
        // Handlers must be Clone; both captures are shared through an Arc for exactly that.
        let respond = Arc::new(respond);
        let app = Router::new().route(
            "/{*rest}",
            any(
                move |Path(path): Path<String>, headers: HeaderMap, body: axum::body::Bytes| {
                    let respond = Arc::clone(&respond);
                    let state = Arc::clone(&state);
                    async move {
                        // The Bot API methods this crate exposes carry JSON bodies — except
                        // setWebhook, which is always multipart because of its optional
                        // certificate field. JSON is parsed when parseable; the raw bytes and the
                        // framing content type are always kept for the multipart assertions.
                        let raw_bytes = body.clone();
                        let content_type = headers
                            .get(header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_owned();
                        let parsed = if body.is_empty() {
                            None
                        } else {
                            serde_json::from_slice::<Value>(&body).ok()
                        };
                        let entry = Captured {
                            path: format!("/{path}"),
                            body: parsed,
                            raw: (content_type, raw_bytes),
                        };
                        let response = respond(&entry);
                        state.lock().expect("capture lock").push(entry);
                        response
                    }
                },
            ),
        );
        // Bind inside the serving runtime: a `tokio::net::TcpListener` belongs to the runtime
        // whose driver registered it, so binding on the test's runtime and serving on another
        // thread's runtime hands the accept loop IO it cannot poll. One thread owns bind, serve,
        // and shutdown together; `spawn_raw` returns only after the port is bound.
        let (bound_tx, bound_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("harness runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind");
                let bound = listener.local_addr().expect("local addr");
                bound_tx
                    .send(bound)
                    .expect("the test receives the bound address");
                let _ = axum::serve(listener, app).into_future().await;
            });
        });
        let bound = bound_rx.recv().expect("the harness binds before returning");
        Self {
            base_url: Url::parse(&format!("http://{bound}")).expect("base url"),
            captured,
        }
    }

    fn requests(&self) -> Vec<Captured> {
        self.captured.lock().expect("capture lock").clone()
    }
}

fn fixture(raw: &str) -> Value {
    serde_json::from_str(raw).expect("a committed fixture must be valid JSON")
}

fn client(base_url: &Url) -> Client {
    Client::new(
        &SecretString::new(TOKEN.into()),
        base_url,
        Duration::from_secs(5),
    )
    .expect("the harness client must build")
}

fn ok_result(result: &Value) -> Value {
    json!({"ok": true, "result": result})
}

/// `get_me` resolves the bot identity from a recorded response.
#[tokio::test]
async fn get_me_returns_the_bot_identity() {
    let harness = Harness::spawn({
        let me = fixture(include_str!("fixtures/get_me.json"));
        move |_| me.clone()
    })
    .await;
    let me = client(&harness.base_url)
        .get_me()
        .await
        .expect("get_me must resolve");

    assert_eq!(me.user.id.0, 700_100_200);
    assert_eq!(me.username(), "ratatoskr_test_bot");

    let captured = &harness.requests()[0];
    assert_eq!(captured.path, format!("/bot{TOKEN}/GetMe"));
    // Parameterless Bot API methods still POST an empty JSON object.
    assert_eq!(
        captured.body.as_ref(),
        Some(&serde_json::json!({})),
        "getMe carries no fields"
    );
}

/// A Bot API error body surfaces as the api class carrying Telegram's description — and never
/// carrying the token.
#[tokio::test]
async fn an_api_error_surfaces_as_its_class_without_the_token() {
    let harness = Harness::spawn({
        let error = fixture(include_str!("fixtures/api_error.json"));
        move |_| error.clone()
    })
    .await;
    let failure = client(&harness.base_url)
        .send_message(ChatId(900_700_601), "synthetic text", None)
        .await
        .expect_err("an API error must fail the call");

    match failure {
        BotApiError::Api { description } => {
            assert!(description.contains("chat not found"), "{description}");
            assert!(
                !description.contains(TOKEN),
                "the token leaked into the description"
            );
        }
        other => panic!("expected Api, got {other:?}"),
    }
    // The failed call went to the right place anyway.
    assert_eq!(
        harness.requests()[0].path,
        format!("/bot{TOKEN}/SendMessage")
    );
}

/// A 429 with `retry_after` surfaces as the `rate_limited` class carrying that delay.
#[tokio::test]
async fn a_rate_limited_answer_carries_its_retry_delay() {
    let harness = Harness::spawn({
        let limited = fixture(include_str!("fixtures/rate_limited.json"));
        move |_| limited.clone()
    })
    .await;
    let failure = client(&harness.base_url)
        .send_message(ChatId(900_700_601), "synthetic text", None)
        .await
        .expect_err("429 must fail the call");
    match failure {
        BotApiError::RateLimited { retry_after } => {
            assert_eq!(retry_after, Duration::from_secs(7));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

/// An unreachable endpoint is a network failure, not a panic and not an api error.
#[tokio::test]
async fn an_unreachable_endpoint_is_a_network_failure() {
    // Bind and drop: the port is closed again immediately.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);

    let base_url = Url::parse(&format!("http://127.0.0.1:{port}")).expect("url");
    let failure = client(&base_url)
        .get_me()
        .await
        .expect_err("closed port must fail");
    assert!(
        matches!(failure, BotApiError::Network(_)),
        "expected Network, got {failure:?}",
    );
}

/// A call slower than the configured timeout is a network failure rather than a hang.
#[tokio::test]
async fn a_call_beyond_the_timeout_is_a_network_failure() {
    let harness = Harness::spawn(|_| {
        std::thread::sleep(Duration::from_secs(1));
        ok_result(&json!(true))
    })
    .await;
    let slow = Client::new(
        &SecretString::new(TOKEN.into()),
        &harness.base_url,
        Duration::from_millis(100),
    )
    .expect("the harness client must build");
    let failure = slow
        .get_me()
        .await
        .expect_err("the timeout must fail the call");
    assert!(matches!(failure, BotApiError::Network(_)), "{failure:?}");
}

/// `send_message` posts its typed payload to the harness.
#[tokio::test]
async fn send_message_posts_its_typed_payload() {
    let harness = Harness::spawn(move |_| {
        ok_result(&fixture(include_str!("fixtures/message.json"))["message"])
    })
    .await;
    let message = client(&harness.base_url)
        .send_message(ChatId(900_700_602), "synthetic text", None)
        .await
        .expect("send_message must deliver");
    assert_eq!(message.id.0, 55);

    let captured = &harness.requests()[0];
    assert_eq!(captured.path, format!("/bot{TOKEN}/SendMessage"));
    let body = captured.body.clone().expect("sendMessage carries a body");
    assert_eq!(body["chat_id"], 900_700_602);
    assert_eq!(body["text"], "synthetic text");
}

/// `edit_message_text` addresses one message by chat and message id.
#[tokio::test]
async fn edit_message_text_addresses_one_message() {
    let harness =
        Harness::spawn(|_| ok_result(&fixture(include_str!("fixtures/message.json"))["message"]))
            .await;
    client(&harness.base_url)
        .edit_message_text(ChatId(900_700_602), MessageId(55), "edited text", None)
        .await
        .expect("edit_message_text must deliver");

    let body = harness.requests()[0].body.clone().expect("body");
    assert_eq!(body["chat_id"], 900_700_602);
    assert_eq!(body["message_id"], 55);
    assert_eq!(body["text"], "edited text");
}

/// `answer_callback_query` names only the query id it answers.
#[tokio::test]
async fn answer_callback_query_names_the_query_id() {
    let harness = Harness::spawn(|_| ok_result(&json!(true))).await;
    client(&harness.base_url)
        .answer_callback_query("4382fecwq")
        .await
        .expect("answer_callback_query must deliver");

    let body = harness.requests()[0].body.clone().expect("body");
    assert_eq!(body["callback_query_id"], "4382fecwq");
}

/// `send_chat_action` posts the typed action's wire name.
#[tokio::test]
async fn send_chat_action_posts_the_wire_action_name() {
    let harness = Harness::spawn(|_| ok_result(&json!(true))).await;
    client(&harness.base_url)
        .send_chat_action(ChatId(900_700_602), ChatAction::Typing)
        .await
        .expect("send_chat_action must deliver");

    let body = harness.requests()[0].body.clone().expect("body");
    assert_eq!(body["chat_id"], 900_700_602);
    assert_eq!(body["action"], "typing");
}

/// `send_message` and `edit_message_text` carry parse mode and reply markup when given, and
/// neither field when not.
#[tokio::test]
async fn send_and_edit_carry_parse_mode_and_reply_markup() {
    let harness =
        Harness::spawn(|_| ok_result(&fixture(include_str!("fixtures/message.json"))["message"]))
            .await;
    let markup = json!({
        "inline_keyboard": [[{"text": "Open", "url": "https://t.me/ratatoskr_test_bot"}]]
    });
    let options = MessageOptions {
        parse_mode: Some("HTML".to_owned()),
        reply_markup: Some(markup.clone()),
    };
    client(&harness.base_url)
        .send_message(ChatId(900_700_602), "<b>done</b>", Some(&options))
        .await
        .expect("send with options");

    let body = harness.requests()[0].body.clone().expect("body");
    assert_eq!(body["parse_mode"], "HTML");
    assert_eq!(body["reply_markup"], markup);

    client(&harness.base_url)
        .edit_message_text(
            ChatId(900_700_602),
            MessageId(55),
            "<i>still done</i>",
            Some(&options),
        )
        .await
        .expect("edit with options");
    let body = harness.requests()[1].body.clone().expect("body");
    assert_eq!(body["parse_mode"], "HTML");
    assert_eq!(body["reply_markup"], markup);

    // Without options neither field may appear on the wire.
    client(&harness.base_url)
        .send_message(ChatId(900_700_602), "plain", None)
        .await
        .expect("plain send");
    let body = harness.requests()[2].body.clone().expect("body");
    assert!(body.get("parse_mode").is_none(), "{body}");
    assert!(body.get("reply_markup").is_none(), "{body}");
}

/// `set_webhook` carries the registration URL and the admission secret as multipart form fields —
/// teloxide frames this method as multipart because of its optional certificate parameter.
#[tokio::test]
async fn set_webhook_carries_the_url_and_the_secret() {
    let harness = Harness::spawn(|_| ok_result(&json!(true))).await;
    let webhook_secret = SecretString::new("webhook-secret-0123456789abcdef".into());
    client(&harness.base_url)
        .set_webhook(
            &Url::parse("https://example.test/webhook").expect("webhook url"),
            Some(&webhook_secret),
        )
        .await
        .expect("set_webhook must deliver");

    let captured = &harness.requests()[0];
    assert_eq!(captured.path, format!("/bot{TOKEN}/SetWebhook"));
    assert!(
        captured.raw.0.starts_with("multipart/form-data"),
        "{}",
        captured.raw.0
    );
    assert_eq!(
        captured.form_field("url").as_deref(),
        Some("https://example.test/webhook"),
    );
    assert_eq!(
        captured.form_field("secret_token").as_deref(),
        Some(webhook_secret.expose_secret()),
    );
}

/// `get_file` resolves a synthetic file id into the served metadata.
#[tokio::test]
async fn get_file_resolves_the_harness_file_metadata() {
    let harness = Harness::spawn({
        let file = fixture(include_str!("fixtures/get_file.json"));
        move |_| file.clone()
    })
    .await;
    let resolved: File = client(&harness.base_url)
        .get_file("attach-doc-1")
        .await
        .expect("get_file must resolve");

    assert_eq!(resolved.path, "documents/report.pdf");
    assert_eq!(resolved.size, 27);

    let captured = &harness.requests()[0];
    assert_eq!(captured.path, format!("/bot{TOKEN}/GetFile"));
    let body = captured.body.clone().expect("getFile carries a body");
    assert_eq!(body["file_id"], "attach-doc-1");
}

/// The bounded download streams exactly the bytes the harness serves, through the
/// token-bearing file route, without ever materializing them outside the stream.
#[tokio::test]
async fn download_streams_exactly_the_served_bytes() {
    const PAYLOAD: &[u8] = b"ratatoskr harness file payload bytes";
    let harness = Harness::spawn_raw(move |captured| {
        if captured.path.starts_with("/file/") {
            axum::body::Bytes::from_static(PAYLOAD).into_response()
        } else {
            Json(ok_result(&json!(true))).into_response()
        }
    })
    .await;

    let mut stream: DownloadStream = client(&harness.base_url)
        .download_file("documents/report.pdf")
        .await
        .expect("the download must start");
    let mut received = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut received)
        .await
        .expect("the stream must read to end");
    assert_eq!(received, PAYLOAD);

    // The download addressed this deployment's own bot over the `/file` origin.
    let requests = harness.requests();
    let paths: Vec<&str> = requests.iter().map(|c| c.path.as_str()).collect();
    assert!(
        paths.contains(&format!("/file/bot{TOKEN}/documents/report.pdf").as_str()),
        "{paths:?}"
    );
}
