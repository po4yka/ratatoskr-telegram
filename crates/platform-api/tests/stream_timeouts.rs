//! R5-10: the Platform client's timeouts. The progress stream is bounded by an idle (read) timeout
//! and not by the whole-call timeout, while request/response calls keep the whole-call timeout.
//! Every test runs against a raw local TCP server that scripts exactly when bytes arrive.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use platform_api::{Client, PlatformError};
use url::Url;

/// A session credential no Platform ever issued.
const SESSION: &str = "test-session-credential";

/// What a timed raw server does after it has sent the response head.
#[derive(Clone, Copy)]
enum AfterHead {
    /// Send `keep_alives` comments, one every `every`, then the terminal frame, then close.
    KeepAlivesThenTerminal { keep_alives: u32, every: Duration },
    /// Send one comment and then nothing at all, holding the connection open.
    SilentAfterOneComment,
    /// Send nothing, not even the response head.
    NoHead,
}

/// A raw TCP server for timing behaviour: one thread per connection, every accepted connection
/// counted and every request's header block recorded.
struct TimedServer {
    base_url: Url,
    connections: Arc<std::sync::atomic::AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
}

const TERMINAL_ID: &str = "018f0000-0000-7000-8000-00000000cc01";

/// Read the request head, record it, then play `behaviour` on the connection. Returns early on
/// any socket error: the client hanging up is how these tests end a connection.
fn serve_timed_connection(
    mut socket: std::net::TcpStream,
    behaviour: AfterHead,
    log: &Mutex<Vec<String>>,
) {
    use std::io::{Read as _, Write as _};

    let mut received = Vec::new();
    let mut buffer = [0u8; 4096];
    while !received.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = socket.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            return;
        }
        received.extend_from_slice(buffer.get(..read).unwrap_or(&[]));
    }
    log.lock()
        .expect("request log")
        .push(String::from_utf8_lossy(&received).to_string());
    if matches!(behaviour, AfterHead::NoHead) {
        std::thread::sleep(Duration::from_secs(8));
        return;
    }
    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
    if socket.write_all(head.as_bytes()).is_err() {
        return;
    }
    match behaviour {
        AfterHead::KeepAlivesThenTerminal { keep_alives, every } => {
            for _ in 0..keep_alives {
                if socket.write_all(b": keep-alive\n\n").is_err() {
                    return;
                }
                let _ = socket.flush();
                std::thread::sleep(every);
            }
            let terminal = format!(
                "id: {TERMINAL_ID}\nevent: progress\ndata: {{\"status\":\"succeeded\",\"observed_at\":\"2026-10-12T10:00:00Z\"}}\n\n"
            );
            let _ = socket.write_all(terminal.as_bytes());
        }
        AfterHead::SilentAfterOneComment => {
            let _ = socket.write_all(b": keep-alive\n\n");
            let _ = socket.flush();
            std::thread::sleep(Duration::from_secs(8));
        }
        AfterHead::NoHead => {}
    }
}

fn spawn_timed_server(behaviour: AfterHead) -> TimedServer {
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let connections = Arc::new(AtomicUsize::new(0));
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let listener = TcpListener::bind("127.0.0.1:0").expect("timed bind");
    let port = listener.local_addr().expect("timed addr").port();
    let counted = Arc::clone(&connections);
    let log = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(socket) = stream else { break };
            counted.fetch_add(1, Ordering::SeqCst);
            let log = Arc::clone(&log);
            std::thread::spawn(move || serve_timed_connection(socket, behaviour, &log));
        }
    });
    TimedServer {
        base_url: Url::parse(&format!("http://127.0.0.1:{port}")).expect("timed url"),
        connections,
        requests,
    }
}

fn operation() -> uuid::Uuid {
    "018f0000-0000-7000-8000-00000000beef"
        .parse()
        .expect("uuid")
}

/// R5-10: a stream that keeps receiving keep-alives outlives the whole-call timeout and delivers
/// its terminal frame over one connection, never reporting a timeout.
#[tokio::test]
async fn a_stream_with_keep_alives_outlives_the_call_timeout_on_one_connection() {
    let server = spawn_timed_server(AfterHead::KeepAlivesThenTerminal {
        keep_alives: 15,
        every: Duration::from_millis(100),
    });
    let client = Client::with_timeouts(
        &server.base_url,
        Duration::from_millis(500),
        Duration::from_secs(2),
    )
    .expect("the client must build");
    let mut stream = client
        .stream_events(SESSION, operation(), None)
        .await
        .expect("the stream must open");

    let frame = stream
        .next_frame()
        .await
        .expect("the stream must not report a timeout while keep-alives arrive")
        .expect("the terminal frame must be delivered");
    assert_eq!(frame.event_id.to_string(), TERMINAL_ID);
    assert_eq!(frame.status, platform_api::OperationStatus::Succeeded);
    assert!(
        stream.next_frame().await.expect("stream end").is_none(),
        "nothing follows the terminal frame"
    );
    assert_eq!(
        server.connections.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the whole follow must use one connection"
    );
}

/// R5-10: a stream that goes silent ends after the idle timeout, as the typed timeout error that
/// the follower counts as one clean close, long before the whole-call timeout.
#[tokio::test]
async fn a_silent_stream_ends_after_the_idle_timeout() {
    let server = spawn_timed_server(AfterHead::SilentAfterOneComment);
    let client = Client::with_timeouts(
        &server.base_url,
        Duration::from_secs(30),
        Duration::from_millis(300),
    )
    .expect("the client must build");
    let mut stream = client
        .stream_events(SESSION, operation(), None)
        .await
        .expect("the stream must open");

    let outcome = tokio::time::timeout(Duration::from_secs(3), stream.next_frame())
        .await
        .expect("the idle timeout must end the stream before the call timeout");
    assert!(
        matches!(outcome, Err(PlatformError::Timeout)),
        "an idle stream ends with the typed timeout error, got {outcome:?}"
    );
}

/// R5-10: a server that accepts the request and never answers is cut by the idle timeout too, so
/// a stream without a total timeout cannot hang while waiting for its response head.
#[tokio::test]
async fn a_stream_whose_head_never_arrives_is_cut_by_the_idle_timeout() {
    let server = spawn_timed_server(AfterHead::NoHead);
    let client = Client::with_timeouts(
        &server.base_url,
        Duration::from_secs(30),
        Duration::from_millis(300),
    )
    .expect("the client must build");

    let outcome = tokio::time::timeout(
        Duration::from_secs(3),
        client.stream_events(SESSION, operation(), None),
    )
    .await
    .expect("the idle timeout must end the wait for the response head");
    assert!(
        matches!(outcome, Err(PlatformError::Timeout)),
        "a missing head ends with the typed timeout error, got {:?}",
        outcome.map(|_| ())
    );
}

/// A call cut by its timeout: the typed class, or the transport error of a body that stalled
/// after the head (reqwest reports that one as a body error whose cause is a timeout).
fn timed_out(error: &PlatformError) -> bool {
    match error {
        PlatformError::Timeout => true,
        PlatformError::Network(inner) => inner.is_timeout(),
        _ => false,
    }
}

/// R5-10: request/response calls keep the whole-call timeout: a body that stalls is cut by it.
#[tokio::test]
async fn a_stalled_request_response_call_is_still_cut_by_the_call_timeout() {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("stall bind");
    let port = listener.local_addr().expect("stall addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut socket) = stream else { break };
            std::thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                let _ = socket.read(&mut buffer);
                let _ = socket.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 400\r\n\r\n{\"status\":",
                );
                let _ = socket.flush();
                std::thread::sleep(Duration::from_secs(8));
            });
        }
    });
    let base_url = Url::parse(&format!("http://127.0.0.1:{port}")).expect("stall url");
    // An idle bound far above the call bound proves the call bound is what cuts the call.
    let client = Client::with_timeouts(
        &base_url,
        Duration::from_millis(300),
        Duration::from_secs(45),
    )
    .expect("the client must build");

    let read = tokio::time::timeout(
        Duration::from_secs(3),
        client.read_operation(SESSION, operation()),
    )
    .await
    .expect("the call timeout must cut a stalled body");
    assert!(
        read.as_ref().is_err_and(timed_out),
        "read_operation must be cut by the call timeout, got {:?}",
        read.map(|_| ())
    );
    let exchange = tokio::time::timeout(Duration::from_secs(3), client.exchange_assertion("a.b.c"))
        .await
        .expect("the call timeout must cut a stalled body");
    assert!(
        exchange.as_ref().is_err_and(timed_out),
        "exchange_assertion must be cut by the call timeout, got {:?}",
        exchange.map(|_| ())
    );
}

/// R5-10: a resumed stream still names the last seen event id.
#[tokio::test]
async fn a_resumed_stream_still_sends_last_event_id() {
    let server = spawn_timed_server(AfterHead::KeepAlivesThenTerminal {
        keep_alives: 1,
        every: Duration::from_millis(10),
    });
    let client = Client::with_timeouts(
        &server.base_url,
        Duration::from_millis(500),
        Duration::from_secs(2),
    )
    .expect("the client must build");
    let resume = "018f0000-0000-7000-8000-00000000cc00";
    let mut stream = client
        .stream_events(SESSION, operation(), Some(resume))
        .await
        .expect("the resumed stream must open");
    let _ = stream.next_frame().await.expect("terminal frame");

    let request = server
        .requests
        .lock()
        .expect("request log")
        .first()
        .expect("one request")
        .to_ascii_lowercase();
    assert!(
        request.contains(&format!("last-event-id: {resume}")),
        "the resume must carry Last-Event-ID, got:\n{request}"
    );
}
