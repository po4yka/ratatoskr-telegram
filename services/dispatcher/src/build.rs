//! The startup factory: from validated configuration to running dispatcher workers.
//!
//! The lifecycle calls [`build`] once, after the database is prepared and before any listener
//! binds. It builds the Bot API client, the delivery limiter, the sender, and the projection
//! consumer over shared handles, then returns every worker to the shared process lifecycle:
//!
//! - **the sender** drains the durable queue until cancellation
//!   ([`OutboundSender::run_until_shutdown`]), sampling the queue-depth gauge once per cycle;
//! - **the consumer** accepts operation events pushed into a bounded feed channel
//!   ([`PROJECTION_FEED_CAPACITY`]) through [`DispatcherRuntime::projection_feed`].
//!
//! The feed is the transport seam (design D6). Shutdown closes admission, drains accepted events,
//! and joins every task before the database and telemetry are closed.

use std::sync::Arc;
use std::time::Duration;

use secrecy::ExposeSecret as _;
use telegram_core::Subsystem;
use telegram_core::TelegramError;
use telegram_core::config::DispatcherConfig;
use telegram_http::{BackgroundRuntime, PublicContext};
use telegram_persistence::Database;

use crate::outbound::clock::{Clock, SystemClock};
use crate::outbound::limiter::DeliveryLimiter;
use crate::outbound::sender::{ClientSink, OutboundSender, SenderLimits};
use crate::projection::consumer::ProjectionConsumer;
use crate::projection::event::OperationEvent;

/// How many accepted-but-unconsumed operation events the in-process feed may hold.
///
/// Bounded for the same reason every other handoff here is: a publisher that outruns the
/// database must feel backpressure instead of buying memory. The durable queue behind the
/// consumer is the real buffer; this channel is only a wake-up pipe into it.
pub const PROJECTION_FEED_CAPACITY: usize = 1024;

/// The handles one running dispatcher keeps: everything a publisher needs to reach the workers.
pub struct DispatcherRuntime {
    projection_feed: tokio::sync::mpsc::Sender<OperationEvent>,
    background: BackgroundRuntime,
}

impl DispatcherRuntime {
    /// The feed operation events are pushed into. Cloning is cheap and intended: every publisher
    /// holds its own handle, and the workers stay alive while at least one exists.
    #[must_use]
    pub fn projection_feed(&self) -> tokio::sync::mpsc::Sender<OperationEvent> {
        self.projection_feed.clone()
    }

    /// Stop new work admission without interrupting an already admitted delivery transaction.
    pub fn request_shutdown(&mut self) {
        self.background.request_shutdown();
    }

    /// Request shutdown and wait for every owned worker.
    pub async fn join(mut self) {
        self.background.request_shutdown();
        self.background.join().await;
    }

    /// Hand every owned task to the shared HTTP lifecycle.
    #[must_use]
    pub fn into_background_runtime(self) -> BackgroundRuntime {
        let Self {
            projection_feed,
            background,
        } = self;
        drop(projection_feed);
        background
    }

    fn spawn_owned<F>(&mut self, worker: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.background.spawn(worker);
    }
}

/// Build the dispatcher's workers from the validated configuration, or refuse startup.
///
/// # Errors
///
/// A [`TelegramError::Internal`] labelled `http` when the prepared database is somehow absent —
/// unreachable behind validation V15 — or `bot_api` when the client stack cannot be built.
///
pub async fn build(context: PublicContext) -> Result<BackgroundRuntime, TelegramError> {
    let database = context.database.ok_or_else(|| {
        TelegramError::internal(
            Subsystem::Http,
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the dispatcher database was not prepared",
            ),
        )
    })?;

    let bot_api = &context.config.bot_api;
    let client = bot_api::Client::new(
        &bot_api.token,
        &bot_api.base_url,
        Duration::from_secs(bot_api.timeout_seconds),
    )
    .map_err(|error| TelegramError::internal(Subsystem::BotApi, error))?;
    let me = client
        .get_me()
        .await
        .map_err(|error| TelegramError::internal(Subsystem::BotApi, error))?;
    let bot_id = i64::try_from(me.user.id.0)
        .map_err(|error| TelegramError::internal(Subsystem::BotApi, error))?;

    context.runtime.mark_notification_configured();

    // The Platform half: sessions for the follower, built once. Startup performs no network call
    // here - the session exchange happens lazily on the first follow.
    let platform = &context.config.platform;
    let platform_client = platform_api::Client::new(
        &platform.base_url,
        Duration::from_secs(platform.timeout_seconds),
    )
    .map_err(|error| TelegramError::internal(Subsystem::Platform, error))?;
    let issuer = platform_api::assertion::AssertionIssuer::from_seed(
        &decode_seed(platform.assertion_signing_key.expose_secret())?,
        &platform.audience,
    )
    .map_err(|error| TelegramError::internal(Subsystem::Platform, error))?;
    let sessions = Arc::new(platform_api::session::SessionSource::new(
        platform_client,
        issuer,
        Box::new(PlatformClock),
    ));

    let username = bot_api.username.clone();
    let mut runtime = spawn_runtime_with(
        context.config.dispatcher,
        &database,
        client,
        Some(Arc::clone(&sessions)),
        username,
    );

    let notification_shutdown = runtime.background.cancel_receiver();
    runtime.spawn_owned(crate::notifications::supervise_until_shutdown(
        context.config.notification_bus.clone(),
        database.clone(),
        bot_id,
        Arc::clone(&context.runtime),
        notification_shutdown,
    ));

    tracing::info!(
        feed_capacity = PROJECTION_FEED_CAPACITY,
        "dispatcher workers started",
    );
    Ok(runtime.into_background_runtime())
}

fn decode_seed(hex_key: &str) -> Result<[u8; 32], TelegramError> {
    let digit = |character: u8| match character {
        b'0'..=b'9' => Some(character - b'0'),
        b'a'..=b'f' => Some(character - b'a' + 10),
        _ => None,
    };
    let bytes = hex_key.as_bytes();
    let byte_len_ok = bytes.len() == 64;
    let hex_ok = bytes.iter().all(|b| digit(*b).is_some());
    if !byte_len_ok || !hex_ok {
        return Err(TelegramError::internal(
            Subsystem::Platform,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the assertion signing key must be 64 hex characters",
            ),
        ));
    }
    let mut seed = [0u8; 32];
    for (slot, pair) in seed.iter_mut().zip(bytes.chunks_exact(2)) {
        if let [high, low] = pair {
            let high = digit(*high).unwrap_or(0);
            let low = digit(*low).unwrap_or(0);
            *slot = (high << 4) | low;
        }
    }
    Ok(seed)
}

/// Assemble the workers over prepared components and spawn them. Exposed so tests can drive the
/// exact production composition without a process.
#[must_use]
pub fn spawn_runtime(
    config: DispatcherConfig,
    database: &Database,
    client: bot_api::Client,
) -> DispatcherRuntime {
    spawn_runtime_with(config, database, client, None, None)
}

/// The full composition: as [`spawn_runtime`], plus the Platform half production wires - the
/// follower's session source and the bot username the terminal composer uses.
#[must_use]
pub fn spawn_runtime_with(
    config: DispatcherConfig,
    database: &Database,
    client: bot_api::Client,
    sessions: Option<Arc<platform_api::session::SessionSource>>,
    bot_username: Option<String>,
) -> DispatcherRuntime {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let limiter = Arc::new(DeliveryLimiter::new(
        config.global_messages_per_second,
        config.per_chat_min_interval_ms,
    ));
    let sink = Arc::new(ClientSink::new(client));
    let sender = OutboundSender::new(
        Arc::new(database.clone()),
        sink,
        limiter,
        Arc::clone(&clock),
        SenderLimits {
            max_attempts: config.max_attempts,
            backoff_base_secs: config.backoff_base_secs,
            backoff_cap_secs: config.backoff_cap_secs,
            jitter_fraction_milli: config.jitter_fraction_milli,
            lease_ttl_secs: config.lease_ttl_secs,
        },
    );
    let consumer = ProjectionConsumer::new(
        database.clone(),
        clock,
        config.render_interval_secs,
        bot_username,
    );

    let (feed, receiver) = tokio::sync::mpsc::channel(PROJECTION_FEED_CAPACITY);
    let (cancel, shutdown) = tokio::sync::watch::channel(false);
    let mut background = BackgroundRuntime::from_tasks(cancel, Vec::new());
    let sender_admission = background.admission_fence();
    let sender_admission_closed = background.admission_closed();
    background.spawn(sender_until_shutdown(
        sender,
        config.poll_idle_ms,
        shutdown.clone(),
        sender_admission,
        sender_admission_closed,
    ));
    background.spawn(consume_until_shutdown(consumer, receiver, shutdown.clone()));
    let follower = sessions.map(|sessions| {
        crate::follow::OperationFollower::new(database.clone(), feed.clone(), sessions)
    });
    if let Some(follower) = follower {
        let follower_admission = background.admission_fence();
        let follower_admission_closed = background.admission_closed();
        background.spawn(follower.run_until_shutdown(
            shutdown,
            follower_admission,
            follower_admission_closed,
        ));
    }
    DispatcherRuntime {
        projection_feed: feed,
        background,
    }
}

/// The sender's owned process lifetime, ending only after cancellation and admitted settlement.
async fn sender_until_shutdown(
    sender: OutboundSender,
    poll_idle_ms: u64,
    shutdown: tokio::sync::watch::Receiver<bool>,
    admission: Arc<tokio::sync::RwLock<()>>,
    admission_closed: Arc<std::sync::atomic::AtomicBool>,
) {
    // The sender half is held for the task's whole life; dropping it would close the channel and
    // end the sender loop as if shutdown had been signalled.
    let (_wake_keep_alive, wake) = tokio::sync::mpsc::channel::<()>(1);
    let idle_poll = Duration::from_millis(poll_idle_ms);
    sender
        .run_until_shutdown(wake, idle_poll, shutdown, admission, admission_closed)
        .await;
}

/// The consumer's owned process lifetime: cancellation closes admission, then drains buffered
/// events before returning.
async fn consume_until_shutdown(
    consumer: ProjectionConsumer,
    mut receiver: tokio::sync::mpsc::Receiver<OperationEvent>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        let event = tokio::select! {
            biased;
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    receiver.close();
                    receiver.recv().await
                } else {
                    continue;
                }
            }
            event = receiver.recv() => event,
        };
        match event {
            Some(event) => match consumer.accept(&event).await {
                Ok(outcome) => tracing::info!(
                    outcome = outcome.as_str(),
                    class = "projection_event",
                    "an operation event was consumed",
                ),
                Err(error) => tracing::error!(
                    error = %error,
                    class = "projection_accept_failed",
                    "an operation event could not be accepted",
                ),
            },
            None => break,
        }
    }
}

/// Bridge the dispatcher's clock trait onto the platform crate's, so both share one injected
/// source in tests and the process clock in production.
#[derive(Debug, Default, Clone, Copy)]
struct PlatformClock;

impl platform_api::session::Clock for PlatformClock {
    fn now(&self) -> jiff::Timestamp {
        jiff::Timestamp::now()
    }
}

impl std::fmt::Debug for DispatcherRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DispatcherRuntime")
            .field("feed_capacity", &PROJECTION_FEED_CAPACITY)
            .finish_non_exhaustive()
    }
}
