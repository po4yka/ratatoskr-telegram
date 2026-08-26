//! The startup factory: from validated configuration to running dispatcher workers.
//!
//! The lifecycle calls [`build`] once, after the database is prepared and before any listener
//! binds. It builds the Bot API client, the delivery limiter, the sender, and the projection
//! consumer over shared handles, then spawns two detached workers:
//!
//! - **the sender** drains the durable queue forever ([`OutboundSender::run_forever`]), sampling
//!   the queue-depth gauge once per cycle;
//! - **the consumer** accepts operation events pushed into a bounded feed channel
//!   ([`PROJECTION_FEED_CAPACITY`]) through [`DispatcherRuntime::projection_feed`].
//!
//! The feed is the transport seam (design D6): nothing publishes yet — the NATS adapter of the
//! workspace-integration item will hold the feed handle for the process lifetime. Until then a
//! closed feed does NOT stop the consumer: it parks on a pending future rather than dying, so a
//! future publisher can never find the worker gone.

use std::sync::Arc;
use std::time::Duration;

use secrecy::ExposeSecret as _;
use telegram_core::Subsystem;
use telegram_core::TelegramError;
use telegram_core::config::DispatcherConfig;
use telegram_http::PublicContext;
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
    follower: Option<crate::follow::OperationFollower>,
}

impl DispatcherRuntime {
    /// The feed operation events are pushed into. Cloning is cheap and intended: every publisher
    /// holds its own handle, and the workers stay alive while at least one exists.
    #[must_use]
    pub fn projection_feed(&self) -> tokio::sync::mpsc::Sender<OperationEvent> {
        self.projection_feed.clone()
    }

    /// The operation follower, when this runtime owns one. Production runtimes always do;
    /// test compositions may construct without.
    #[must_use]
    pub const fn follower(&self) -> Option<&crate::follow::OperationFollower> {
        self.follower.as_ref()
    }
}

/// Build the dispatcher's workers from the validated configuration, or refuse startup.
///
/// # Errors
///
/// A [`TelegramError::Internal`] labelled `http` when the prepared database is somehow absent —
/// unreachable behind validation V15 — or `bot_api` when the client stack cannot be built.
///
/// Synchronous today because every worker spawns without awaiting; the NATS adapter may make it
/// async again when the feed gains a real publisher.
pub fn build(context: PublicContext) -> Result<(), TelegramError> {
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
    let runtime = spawn_runtime_with(
        context.config.dispatcher,
        &database,
        client,
        Some(Arc::clone(&sessions)),
        username,
    );

    // The follower is the feed's first real producer (design D6 of this change): NATS replaces
    // only this loop's ingress at workspace integration.
    if let Some(follower) = runtime.follower() {
        tokio::spawn(follower.clone().run());
    }
    drop(runtime);

    tracing::info!(
        feed_capacity = PROJECTION_FEED_CAPACITY,
        "dispatcher workers started",
    );
    Ok(())
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
    tokio::spawn(sender_forever(sender, config.poll_idle_ms));
    tokio::spawn(consume_forever(consumer, receiver));
    let follower = sessions.map(|sessions| {
        crate::follow::OperationFollower::new(database.clone(), feed.clone(), sessions)
    });
    DispatcherRuntime {
        projection_feed: feed,
        follower,
    }
}

/// The sender's process lifetime: the wake channel never closes because its half lives inside
/// this task, so only process exit ends the loop.
async fn sender_forever(sender: OutboundSender, poll_idle_ms: u64) {
    // The sender half is held for the task's whole life; dropping it would close the channel and
    // end `run_forever` as if shutdown had been signalled.
    let (_wake_keep_alive, wake) = tokio::sync::mpsc::channel::<()>(1);
    let idle_poll = Duration::from_millis(poll_idle_ms);
    sender.run_forever(wake, idle_poll).await;
}

/// The consumer's process lifetime: a closed feed parks the loop instead of ending it, so a
/// future publisher can never find the worker dead.
async fn consume_forever(
    consumer: ProjectionConsumer,
    mut receiver: tokio::sync::mpsc::Receiver<OperationEvent>,
) {
    loop {
        match receiver.recv().await {
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
            None => std::future::pending::<()>().await,
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
