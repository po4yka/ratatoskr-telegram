//! R5-10: the follower against a Platform that keeps a progress stream open for several times the
//! whole-call timeout.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use platform_api::session::Clock;
use ratatoskr_telegram_dispatcher::follow::OperationFollower;
use uuid::Uuid;

use super::{
    AUDIENCE, HarnessState, SEED, database, run_follower, seed_live, serve_harness,
    session_response,
};

const TERMINAL_FRAME: &str = concat!(
    "id: 018f0000-0000-7000-8000-0000000000b1\n",
    "event: progress\n",
    "data: {\"status\":\"succeeded\",\"observed_at\":\"2026-10-12T10:00:00Z\"}\n\n",
);

fn sessions_with_call_timeout(
    base_url: &str,
    clock: Box<dyn Clock>,
    call_timeout: Duration,
) -> Arc<platform_api::session::SessionSource> {
    let client = platform_api::Client::new(
        &url::Url::parse(base_url).expect("harness url"),
        call_timeout,
    )
    .expect("client builds");
    let issuer = platform_api::assertion::AssertionIssuer::from_seed(&SEED, AUDIENCE)
        .expect("issuer builds");
    Arc::new(platform_api::session::SessionSource::new(
        client, issuer, clock,
    ))
}

/// A `Write` target the tracing subscriber fills, so a test can assert which classes were logged.
#[derive(Clone, Default)]
struct LogSink(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log sink").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// A fake Platform whose stream stays open with a keep-alive comment every 100 ms for 1.5 s (three
/// times the 500 ms call timeout the follower is built with) and then delivers the terminal frame.
fn slow_operation_harness() -> (String, Arc<HarnessState>) {
    let state = Arc::new(HarnessState::default());
    let app = axum::Router::new()
        .route(
            "/v1/sessions/telegram",
            axum::routing::post(|State(state): State<Arc<HarnessState>>| async move {
                state.exchanges.fetch_add(1, Ordering::SeqCst);
                session_response("slow-operation-session", "2030-01-01T00:00:00Z")
            }),
        )
        .route(
            "/v1/operations/{operation_id}/events",
            axum::routing::get(
                |State(state): State<Arc<HarnessState>>,
                 AxumPath(_id): AxumPath<String>| async move {
                    state.opens.fetch_add(1, Ordering::SeqCst);
                    // Fifteen keep-alives 100 ms apart, then the terminal frame.
                    let frames = futures_util::StreamExt::then(
                        futures_util::stream::iter(0u32..16),
                        |tick| async move {
                            let chunk: &'static [u8] = if tick < 15 {
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                b": keep-alive\n\n"
                            } else {
                                TERMINAL_FRAME.as_bytes()
                            };
                            Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(chunk))
                        },
                    );
                    (
                        StatusCode::OK,
                        [("content-type", "text/event-stream")],
                        axum::body::Body::from_stream(frames),
                    )
                },
            ),
        )
        .with_state(Arc::clone(&state));
    (serve_harness(app), state)
}

/// R5-10: an operation that outlives the whole-call timeout several times over is followed on
/// one connection to its terminal frame, with no `follow_frame_failed` log and exactly the
/// requests of one follow (one session exchange, one stream open).
#[tokio::test]
async fn an_operation_that_outlives_the_call_timeout_is_followed_on_one_connection() {
    let sink = LogSink::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(sink.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    let _logs = tracing::subscriber::set_default(subscriber);

    let (base_url, state) = slow_operation_harness();
    let db = database().await;
    let operation = Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0020);
    seed_live(&db, operation).await;

    let (feed_tx, mut feed_rx) = tokio::sync::mpsc::channel(64);
    let sessions = sessions_with_call_timeout(
        &base_url,
        Box::new(platform_api::session::SystemClock),
        Duration::from_millis(500),
    );
    let (shutdown, task) = run_follower(OperationFollower::new(
        db.database.clone(),
        feed_tx,
        sessions,
    ));

    let arrived = tokio::time::timeout(Duration::from_secs(10), feed_rx.recv()).await;
    assert!(
        arrived.is_ok(),
        "the terminal frame must arrive on the first stream; it did not within ten seconds \
         after {} stream opens and {} session exchanges",
        state.opens.load(Ordering::SeqCst),
        state.exchanges.load(Ordering::SeqCst),
    );
    let event = arrived
        .ok()
        .flatten()
        .expect("the feed stays open until the terminal frame");
    assert!(
        event.status.is_terminal(),
        "the first frame is the terminal one"
    );

    shutdown.send_replace(true);
    task.await.expect("the follower joins");

    assert_eq!(
        state.opens.load(Ordering::SeqCst),
        1,
        "one follow is one stream open"
    );
    assert_eq!(
        state.exchanges.load(Ordering::SeqCst),
        1,
        "one follow is one session exchange"
    );
    let logged = String::from_utf8_lossy(&sink.0.lock().expect("log sink")).to_string();
    assert!(
        !logged.contains("follow_frame_failed"),
        "no stream may break while keep-alives arrive:\n{logged}"
    );
}
