//! The operation follower: the transport seam's first real producer.
//!
//! A scan loop watches non-terminal bindings - restart recovery IS the table - opens Platform's
//! per-operation SSE stream for each, maps frames onto the internal projection seam, resumes with
//! `Last-Event-ID`, and stops at terminal states. Authentication rides the same per-sender
//! session source submissions use; the operation's owning Telegram user comes from its deep-link
//! intent record, which every accepted capture wrote.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::Sender;
use uuid::Uuid;

use crate::projection::event::OperationEvent;

/// How long between binding scans. Small enough that a capture's first frames arrive well within
/// a human's patience; large enough that an idle deployment issues no meaningful load.
const SCAN_INTERVAL_SECS: u64 = 5;
/// The most operations followed at once. A single-owner deployment holds tens, not thousands;
/// the bound keeps a pathological backlog from becoming unbounded fan-out.
const MAX_STREAMS: usize = 64;
/// Reconnect pause after a transport failure. One step, not arithmetic: at this cadence a fixed
/// pause is honest and testable where jitter would be theater.
const RESUME_PAUSE_SECS: u64 = 2;

/// Watches live Platform operations and feeds their progress into the projection seam.
#[derive(Clone)]
pub struct OperationFollower {
    // Manual `Debug`: the handles are large and their state is database-shaped, so the derived
    // form would either leak nothing useful or drag Debug bounds through every dependency.
    database: telegram_persistence::Database,
    feed: Sender<OperationEvent>,
    sessions: Arc<platform_api::session::SessionSource>,
    /// Streams already opened, across scans. The scan task is its only writer.
    in_flight: Arc<tokio::sync::Mutex<HashMap<Uuid, bool>>>,
    /// Operations whose follow has ended one way or another this process lifetime; they are
    /// never reopened by a later scan.
    finished: Arc<tokio::sync::Mutex<std::collections::HashSet<Uuid>>>,
}

impl OperationFollower {
    /// Build a follower over the pool, the projection feed, and the session source.
    #[must_use]
    pub fn new(
        database: telegram_persistence::Database,
        feed: Sender<OperationEvent>,
        sessions: Arc<platform_api::session::SessionSource>,
    ) -> Self {
        Self {
            database,
            feed,
            sessions,
            in_flight: Arc::new(tokio::sync::Mutex::default()),
            finished: Arc::new(tokio::sync::Mutex::default()),
        }
    }

    /// Scan and follow until shutdown, owning every per-operation task in one join set.
    pub async fn run_until_shutdown(
        self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
        admission: Arc<tokio::sync::RwLock<()>>,
        admission_closed: Arc<std::sync::atomic::AtomicBool>,
    ) {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            if *shutdown.borrow() || admission_closed.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            self.scan_once(&mut tasks, &shutdown, &admission, &admission_closed)
                .await;
            tokio::select! {
                biased;
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                completed = tasks.join_next(), if !tasks.is_empty() => {
                    let _ = completed;
                }
                () = tokio::time::sleep(Duration::from_secs(SCAN_INTERVAL_SECS)) => {}
            }
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }

    /// Diff the non-terminal bindings against the in-flight set and open what is missing.
    async fn scan_once(
        &self,
        tasks: &mut tokio::task::JoinSet<()>,
        shutdown: &tokio::sync::watch::Receiver<bool>,
        admission: &tokio::sync::RwLock<()>,
        admission_closed: &std::sync::atomic::AtomicBool,
    ) {
        let rows = match sqlx::query_scalar::<_, Uuid>(
            "select distinct operation_id from telegram.message_bindings where not terminal",
        )
        .fetch_all(self.database.pool())
        .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(class = "follow_scan_failed", error = %error, "live bindings could not be read");
                return;
            }
        };

        if *shutdown.borrow() || admission_closed.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }

        let mut inflight = self.in_flight.lock().await;
        let finished = self.finished.lock().await;
        inflight.retain(|_, done| !*done);
        for operation_id in rows {
            let _admission = admission.read().await;
            if *shutdown.borrow() || admission_closed.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            if inflight.len() >= MAX_STREAMS {
                break;
            }
            if inflight.contains_key(&operation_id) || finished.contains(&operation_id) {
                continue;
            }
            inflight.insert(operation_id, false);
            let follower = self.clone();
            tasks.spawn(async move {
                metrics::counter!(
                    telegram_telemetry::metrics::TELEGRAM_OPERATION_FOLLOWS_TOTAL,
                    "event" => "started",
                )
                .increment(1);
                follower.follow_one(operation_id).await;
                follower.finished.lock().await.insert(operation_id);
                follower.in_flight.lock().await.remove(&operation_id);
            });
        }
    }

    /// Whether three consecutive clean closes with no terminal frame mean an operation Platform
    /// is not advancing should stop being polled.
    fn give_up(clean_closes: u32, operation_id: Uuid) -> bool {
        if clean_closes < 3 {
            return false;
        }
        tracing::warn!(
            operation = %operation_id,
            class = "follow_stream_stalled",
            "the stream kept closing without progress; giving up"
        );
        true
    }

    /// Follow one operation until its terminal frame, reconnecting with `Last-Event-ID`.
    async fn follow_one(&self, operation_id: Uuid) {
        let owner = match self
            .database
            .find_operation_intent_owner(operation_id)
            .await
        {
            Ok(Some(owner)) => owner.to_string(),
            Ok(None) => {
                // A binding without an intent predates this flow or lost its row; there is no
                // principal to authenticate as, so following it would be guessing.
                tracing::debug!(
                    operation = %operation_id,
                    class = "follow_no_owner",
                    "no intent names this operation's owner; not followed"
                );
                return;
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    class = "follow_owner_lookup_failed",
                    "the intent lookup failed"
                );
                return;
            }
        };
        let session = match self.sessions.credential(&owner).await {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    class = "follow_session_failed",
                    "a session could not be exchanged"
                );
                return;
            }
        };

        let mut last_event_id: Option<Uuid> = None;
        // Three consecutive clean closes with no terminal frame mean an operation Platform is
        // not advancing; stop rather than poll it forever.
        let mut clean_closes = 0u32;
        loop {
            let resume = last_event_id.map(|id| id.to_string());
            let opened = self
                .sessions
                .client()
                .stream_events(&session, operation_id, resume.as_deref())
                .await;
            let mut stream = match opened {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        class = "follow_open_failed",
                        "the progress stream could not be opened; retrying"
                    );
                    clean_closes += 1;
                    if Self::give_up(clean_closes, operation_id) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_secs(RESUME_PAUSE_SECS)).await;
                    continue;
                }
            };
            match self
                .pump(
                    operation_id,
                    &mut stream,
                    &mut last_event_id,
                    &mut clean_closes,
                )
                .await
            {
                Pump::KeepFollowing => {}
                Pump::Stop | Pump::TerminalThenStop => return,
            }
            tokio::time::sleep(Duration::from_secs(RESUME_PAUSE_SECS)).await;
        }
    }

    /// Drain one opened stream until it ends, breaks, or reaches a terminal frame.
    async fn pump(
        &self,
        operation_id: Uuid,
        stream: &mut platform_api::EventStream,
        last_event_id: &mut Option<Uuid>,
        clean_closes: &mut u32,
    ) -> Pump {
        loop {
            let Some(frame) = stream.next_frame().await.unwrap_or_else(|error| {
                tracing::warn!(
                    error = %error,
                    class = "follow_frame_failed",
                    "the progress stream broke; resuming"
                );
                None
            }) else {
                // Clean close before a terminal frame: a bounded run of these gives up.
                *clean_closes += 1;
                return if Self::give_up(*clean_closes, operation_id) {
                    Pump::Stop
                } else {
                    Pump::KeepFollowing
                };
            };
            *last_event_id = Some(frame.event_id);
            *clean_closes = 0;
            let terminal = frame.status.is_terminal();
            let event = map_frame(operation_id, frame);
            if self.feed.send(event).await.is_err() {
                return Pump::Stop; // the dispatcher is shutting down
            }
            if terminal {
                tracing::debug!(operation = %operation_id, "a followed operation reached terminal");
                return Pump::TerminalThenStop;
            }
        }
    }
}

/// What [`OperationFollower::pump`] decided after draining one stream.
enum Pump {
    /// Reopen the stream and keep following.
    KeepFollowing,
    /// The frame flow ended for good this process lifetime.
    Stop,
    /// A placeholder variant folded into `Stop` by construction above.
    TerminalThenStop,
}

/// Map one wire frame onto the internal projection event. Status vocabularies are identical by
/// contract; the mapping exists so neither side imports the other's types.
#[must_use]
pub(crate) fn map_frame(operation_id: Uuid, frame: platform_api::ProgressFrame) -> OperationEvent {
    use crate::projection::event::OperationStatus;
    OperationEvent {
        event_id: frame.event_id,
        occurred_at_secs: frame.observed_at_secs,
        correlation_id: format!("operation:{operation_id}"),
        operation_id,
        status: match frame.status {
            platform_api::OperationStatus::Accepted => OperationStatus::Accepted,
            platform_api::OperationStatus::Queued => OperationStatus::Queued,
            platform_api::OperationStatus::Running => OperationStatus::Running,
            platform_api::OperationStatus::Succeeded => OperationStatus::Succeeded,
            platform_api::OperationStatus::PartiallySucceeded => {
                OperationStatus::PartiallySucceeded
            }
            platform_api::OperationStatus::Failed => OperationStatus::Failed,
            platform_api::OperationStatus::Cancelled => OperationStatus::Cancelled,
        },
        stage: frame.stage,
        progress_percent: frame.progress_percent,
        errors: Vec::new(),
        warnings: Vec::new(),
        message: frame.message,
    }
}

impl std::fmt::Debug for OperationFollower {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationFollower")
            .finish_non_exhaustive()
    }
}
