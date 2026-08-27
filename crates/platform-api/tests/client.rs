//! The typed Platform client, exercised only against a local harness server.
//!
//! Every test points [`Client`] at an in-process axum server that records what it receives and
//! answers with synthetic bodies shaped like Platform's documented responses. Nothing here
//! contacts a deployed Ratatoskr deployment.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::routing::any;
use platform_api::{CaptureSource, CaptureSubmission, Client, PlatformError};
use serde_json::{Value, json};
use url::Url;

/// A session credential no Platform ever issued.
const SESSION: &str = "test-session-credential";

/// One request the harness captured.
#[derive(Debug, Clone)]
struct Captured {
    path: String,
    headers: HeaderMap,
    body: Option<Value>,
}

impl Captured {
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }
}

/// A local Platform server: answers every call with `respond`, records everything.
struct Harness {
    base_url: Url,
    captured: Arc<Mutex<Vec<Captured>>>,
}

impl Harness {
    async fn spawn(
        respond: impl Fn(&Captured) -> (StatusCode, Value) + Send + Sync + 'static,
    ) -> Self {
        let captured: Arc<Mutex<Vec<Captured>>> = Arc::default();
        let state = Arc::clone(&captured);
        let respond = Arc::new(respond);
        let app = Router::new().route(
            "/{*rest}",
            any(
                move |Path(path): axum::extract::Path<String>, headers: HeaderMap, body: Bytes| {
                    let respond = Arc::clone(&respond);
                    let state = Arc::clone(&state);
                    async move {
                        let entry = Captured {
                            path: format!("/{path}"),
                            headers,
                            body: serde_json::from_slice::<Value>(&body).ok(),
                        };
                        let (status, payload) = respond(&entry);
                        state.lock().expect("capture lock").push(entry);
                        (status, Json(payload))
                    }
                },
            ),
        );
        // Serving runs on a dedicated runtime in a thread of its own, so the harness never nests
        // a block_on inside the test's runtime.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let bound = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("harness runtime");
            let _ = runtime.block_on(axum::serve(listener, app).into_future());
        });
        Self {
            base_url: Url::parse(&format!("http://{bound}")).expect("base url"),
            captured,
        }
    }

    fn requests(&self) -> Vec<Captured> {
        self.captured.lock().expect("capture lock").clone()
    }
}

fn client(base_url: &Url) -> Client {
    Client::new(base_url, Duration::from_secs(5)).expect("the harness client must build")
}

/// A raw TCP server answering every connection with one canned `text/event-stream` response,
/// recording each request's header block. Used where the JSON harness cannot shape a stream.
fn spawn_raw_sse_server(body: String) -> (Url, Arc<Mutex<Vec<String>>>) {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = Arc::clone(&requests);
    let listener = TcpListener::bind("127.0.0.1:0").expect("sse bind");
    let port = listener.local_addr().expect("sse addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut socket) = stream else { break };
            let mut buffer = [0u8; 4096];
            let mut received = Vec::new();
            loop {
                let read = socket.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                received.extend_from_slice(buffer.get(..read).unwrap_or(&[]));
                if received.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            log.lock()
                .expect("request log")
                .push(String::from_utf8_lossy(&received).to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
            );
            let _ = socket.write_all(response.as_bytes());
        }
    });
    (
        Url::parse(&format!("http://127.0.0.1:{port}")).expect("sse url"),
        requests,
    )
}

/// `submit_capture` posts the key, the bearer, and the URL to `/v1/captures`, and resolves to the
/// operation id from the 202 body.
#[tokio::test]
async fn submit_capture_posts_key_bearer_and_url_and_parses_operation() {
    let operation_id = "018f0000-0000-7000-8000-00000000cafe";
    let harness = Harness::spawn(move |_| {
        (
            StatusCode::OK,
            json!({"operation_id": operation_id, "status": "accepted"}),
        )
    })
    .await;
    let accepted = client(&harness.base_url)
        .submit_capture(
            SESSION,
            &CaptureSubmission {
                idempotency_key: "idem-key-1".to_owned(),
                source: CaptureSource::Url("https://example.test/article".to_owned()),
                origin: None,
            },
        )
        .await
        .expect("submission must resolve");

    assert_eq!(accepted.operation_id.to_string(), operation_id);

    let captured = &harness.requests()[0];
    assert_eq!(captured.path, "/v1/captures");
    assert_eq!(captured.header("idempotency-key"), "idem-key-1");
    assert_eq!(
        captured.header("authorization"),
        format!("Bearer {SESSION}")
    );
    assert_eq!(
        captured.body.as_ref(),
        Some(&json!({"url": "https://example.test/article"})),
        "the body carries exactly the submitted address"
    );
}

/// A blob-source capture submits the stored-bytes reference — the `BlobRef` wire shape with the
/// algorithm fixed at sha256 — and never a fabricated URL member.
#[tokio::test]
async fn submit_capture_posts_blob_sources_without_a_url_member() {
    let operation_id = "018f0000-0000-7000-8000-00000000cafe";
    let harness = Harness::spawn(move |_| {
        (
            StatusCode::OK,
            json!({"operation_id": operation_id, "status": "accepted"}),
        )
    })
    .await;
    client(&harness.base_url)
        .submit_capture(
            SESSION,
            &CaptureSubmission {
                idempotency_key: "blob-key-1".to_owned(),
                source: CaptureSource::Blob {
                    owner_service: "ratatoskr-telegram".to_owned(),
                    digest_hex: "ab".repeat(32),
                    media_type: "application/pdf".to_owned(),
                    length_bytes: 1024,
                },
                origin: None,
            },
        )
        .await
        .expect("a blob submission must resolve");

    let captured = &harness.requests()[0];
    let body = captured
        .body
        .clone()
        .expect("the submission carries a body");
    assert_eq!(
        body["blob"],
        json!({
            "owner_service": "ratatoskr-telegram",
            "digest": {"algorithm": "sha256", "hex": "ab".repeat(32)},
            "media_type": "application/pdf",
            "length_bytes": 1024
        }),
        "{body}"
    );
    assert!(
        body.get("url").is_none(),
        "a blob capture must not fabricate an address: {body}"
    );
}

/// Provenance rides additively on URL captures, which stay byte-compatible when absent.
#[tokio::test]
async fn url_captures_carry_origin_additively_and_stay_byte_compatible() {
    let harness = Harness::spawn(move |_| {
        (
            StatusCode::OK,
            json!({"operation_id": "018f0000-0000-7000-8000-00000000cafe", "status": "accepted"}),
        )
    })
    .await;
    let origin = json!({
        "forward": {"kind": "channel", "chat_id": -100_200_300, "message_id": 77,
                     "sent_at_secs": 1_700_000_000}
    });
    client(&harness.base_url)
        .submit_capture(
            SESSION,
            &CaptureSubmission {
                idempotency_key: "origin-key-1".to_owned(),
                source: CaptureSource::Url("https://example.test/story".to_owned()),
                origin: Some(origin.clone()),
            },
        )
        .await
        .expect("the provenance-carrying submission must resolve");
    client(&harness.base_url)
        .submit_capture(
            SESSION,
            &CaptureSubmission {
                idempotency_key: "plain-key-2".to_owned(),
                source: CaptureSource::Url("https://example.test/plain".to_owned()),
                origin: None,
            },
        )
        .await
        .expect("the plain submission must resolve");

    let with_origin = harness.requests()[0].body.clone().expect("body");
    assert_eq!(
        with_origin,
        json!({
            "url": "https://example.test/story",
            "origin": origin
        }),
        "provenance is additive"
    );
    let without_origin = harness.requests()[1].body.clone().expect("body");
    assert!(
        without_origin.get("origin").is_none(),
        "no provenance means no origin member: {without_origin}"
    );
}

/// The snapshot read surfaces the fields rendering needs — status, stage, percentage, message,
/// and the error and warning lines — while ignoring additive producer fields.
#[tokio::test]
async fn read_operation_parses_snapshot_fields() {
    let harness = Harness::spawn(move |_| {
        (
            StatusCode::OK,
            json!({
                "operation_id": "018f0000-0000-7000-8000-00000000beef",
                "kind": "content.capture.submit",
                "status": "failed",
                "accepted_at": "2026-08-17T09:59:00Z",
                "status_changed_at": "2026-08-17T10:05:00Z",
                "retryable": true,
                "stage": "retrieving",
                "progress_percent": 30,
                "message": "the source could not be retrieved",
                "errors": [
                    {"code": "content.source.temporary",
                     "message": "source refused the connection",
                     "retryable": true}
                ],
                "warnings": [
                    {"code": "content.metadata.partial",
                     "message": "some metadata fields were missing"}
                ],
                "results": [{"result_kind": "content.document"}],
                "tenant_id": "user:018f0000-0000-7000-8000-00000000abcd"
            }),
        )
    })
    .await;
    let operation_id: uuid::Uuid = "018f0000-0000-7000-8000-00000000beef"
        .parse()
        .expect("synthetic uuid");
    let view = client(&harness.base_url)
        .read_operation(SESSION, operation_id)
        .await
        .expect("the read must resolve");

    assert_eq!(view.status, platform_api::OperationStatus::Failed);
    assert_eq!(view.stage.as_deref(), Some("retrieving"));
    assert_eq!(view.progress_percent, Some(30));
    assert_eq!(
        view.message.as_deref(),
        Some("the source could not be retrieved")
    );
    assert_eq!(view.errors.len(), 1);
    assert_eq!(view.errors[0].code, "content.source.temporary");
    assert_eq!(view.errors[0].message, "source refused the connection");
    assert_eq!(view.warnings.len(), 1);
    assert_eq!(view.warnings[0].code, "content.metadata.partial");
}

/// The SSE stream yields typed frames in order, resumes with `Last-Event-ID` when asked, and
/// ends at the terminal frame without reading past it.
#[tokio::test]
async fn stream_events_delivers_frames_resumes_and_stops_at_terminal() {
    let id_a = "018f0000-0000-7000-8000-00000000aaa1";
    let id_b = "018f0000-0000-7000-8000-00000000aaa2";
    let id_c = "018f0000-0000-7000-8000-00000000aaa3";
    // Terminal frame is followed by one more entry that must never be yielded.
    let body = format!(
        ": keep-alive comment\r
\r
id: {id_a}\r
event: progress\r
data: {{\"progress_id\":\"{id_a}\",\"status\":\"accepted\",\"observed_at\":\"2026-08-17T10:00:00Z\"}}\r
\r
id: {id_b}\r
event: progress\r
data: {{\"progress_id\":\"{id_b}\",\"status\":\"running\",\"stage\":\"retrieving\",\"progress_percent\":40,\"message\":null,\"observed_at\":\"2026-08-17T10:00:30Z\"}}\r
\r
id: {id_c}\r
event: progress\r
data: {{\"progress_id\":\"{id_c}\",\"status\":\"succeeded\",\"observed_at\":\"2026-08-17T10:01:00Z\"}}\r
\r
id: 018f0000-0000-7000-8000-00000000aaa4\r
event: progress\r
data: {{\"progress_id\":\"018f0000-0000-7000-8000-00000000aaa4\",\"status\":\"running\",\"observed_at\":\"2026-08-17T10:01:30Z\"}}\r
\r
"
    );
    let (sse_base_url, sse_requests) = spawn_raw_sse_server(body);
    let mut stream = client(&sse_base_url)
        .stream_events(
            SESSION,
            "018f0000-0000-7000-8000-00000000beef"
                .parse()
                .expect("uuid"),
            None,
        )
        .await
        .expect("the stream must open");

    let first = stream.next_frame().await.expect("frame a").expect("a");
    assert_eq!(first.event_id.to_string(), id_a);
    assert_eq!(first.status, platform_api::OperationStatus::Accepted);
    assert_eq!(first.observed_at_secs, 1_786_960_800);

    let second = stream.next_frame().await.expect("frame b").expect("b");
    assert_eq!(second.status, platform_api::OperationStatus::Running);
    assert_eq!(second.stage.as_deref(), Some("retrieving"));
    assert_eq!(second.progress_percent, Some(40));

    let third = stream.next_frame().await.expect("frame c").expect("c");
    assert_eq!(third.status, platform_api::OperationStatus::Succeeded);

    let after_terminal = stream.next_frame().await.expect("stream end");
    assert!(after_terminal.is_none(), "nothing may follow the terminal");

    // A reconnect names the last seen id so the server can resume after it.
    let _ = client(&sse_base_url)
        .stream_events(
            SESSION,
            "018f0000-0000-7000-8000-00000000beef"
                .parse()
                .expect("uuid"),
            Some(id_b),
        )
        .await
        .expect("the resumed stream must open");
    let last_request = sse_requests
        .lock()
        .expect("request log")
        .last()
        .expect("the resume opened a second request")
        .clone();
    let expected_resume = format!("last-event-id: {id_b}");
    assert!(
        last_request.contains(&expected_resume),
        "the resume must carry Last-Event-ID: {id_b}, got:\n{last_request}"
    );
}

/// Platform's failure envelope classes surface from the typed methods: the uniform credential
/// refusal, the idempotency conflict, the rate limit, the generic client class, the server
/// class, and a transport-level network class.
#[tokio::test]
async fn platform_error_classes_map_from_status_envelopes() {
    struct Case {
        status: StatusCode,
    }
    let cases = vec![
        Case {
            status: StatusCode::UNAUTHORIZED,
        },
        Case {
            status: StatusCode::FORBIDDEN,
        },
        Case {
            status: StatusCode::CONFLICT,
        },
        Case {
            status: StatusCode::TOO_MANY_REQUESTS,
        },
        Case {
            status: StatusCode::IM_A_TEAPOT,
        },
        Case {
            status: StatusCode::BAD_GATEWAY,
        },
    ];

    for case in cases {
        let expected_status = case.status;
        let harness = Harness::spawn(move |_| {
            (
                expected_status,
                json!({"error": {"code": "synthetic", "message": "synthetic refusal"}}),
            )
        })
        .await;
        // The harness answers every path; read_operation is the probe.
        let operation_id = "018f0000-0000-7000-8000-00000000beef";
        let expected_class = match expected_status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "unauthenticated",
            StatusCode::CONFLICT => "conflict",
            StatusCode::TOO_MANY_REQUESTS => "rate_limited",
            StatusCode::IM_A_TEAPOT => "client_error",
            _ => "server_error",
        };
        let failure = client(&harness.base_url)
            .read_operation(SESSION, operation_id.parse().expect("synthetic uuid"))
            .await
            .expect_err("an error status must fail the call");

        let believed = match &failure {
            PlatformError::Unauthenticated => matches!(
                expected_status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ),
            PlatformError::Conflict => expected_status == StatusCode::CONFLICT,
            PlatformError::RateLimited => expected_status == StatusCode::TOO_MANY_REQUESTS,
            PlatformError::ClientError { status } => {
                expected_status == StatusCode::IM_A_TEAPOT && *status == 418
            }
            PlatformError::ServerError { status } => {
                expected_status == StatusCode::BAD_GATEWAY && *status == 502
            }
            other => panic!("unexpected class for {expected_status}: {other:?}"),
        };
        assert!(
            believed,
            "status {expected_status} must surface as {expected_class}, got {failure:?}"
        );
    }
}
