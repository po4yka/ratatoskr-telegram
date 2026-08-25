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
#[derive(Debug, Clone)]
pub struct DispatcherRuntime {
    projection_feed: tokio::sync::mpsc::Sender<OperationEvent>,
}

impl DispatcherRuntime {
    /// The feed operation events are pushed into. Cloning is cheap and intended: every publisher
    /// holds its own handle, and the workers stay alive while at least one exists.
    #[must_use]
    pub fn projection_feed(&self) -> tokio::sync::mpsc::Sender<OperationEvent> {
        self.projection_feed.clone()
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

    // The workers are detached by design; no publisher exists yet (design D6), so the runtime
    // handle ends its life here and the consumer parks on its closed feed until process exit.
    drop(spawn_runtime(context.config.dispatcher, &database, client));
    tracing::info!(
        feed_capacity = PROJECTION_FEED_CAPACITY,
        "dispatcher workers started",
    );
    Ok(())
}

/// Assemble the workers over prepared components and spawn them. Exposed so tests can drive the
/// exact production composition without a process.
#[must_use]
pub fn spawn_runtime(
    config: DispatcherConfig,
    database: &Database,
    client: bot_api::Client,
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
    let consumer = ProjectionConsumer::new(database.clone(), clock, config.render_interval_secs);

    let (feed, receiver) = tokio::sync::mpsc::channel(PROJECTION_FEED_CAPACITY);
    tokio::spawn(sender_forever(sender, config.poll_idle_ms));
    tokio::spawn(consume_forever(consumer, receiver));
    DispatcherRuntime {
        projection_feed: feed,
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
