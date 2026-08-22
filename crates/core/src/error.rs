//! The Telegram error taxonomy and its bounded subsystem labels.
//!
//! The two-arm split of the sibling services' boundary error is the shape this taxonomy grows into.
//! At this milestone there is no public route and therefore no client-visible rejection arm: a
//! failure kind with no producer is how dead taxonomy grows, so [`TelegramError`] carries only the
//! internal arm, and the rejected arm arrives with the webhook's public surface together with the
//! contract `ErrorCode`/`SafeMessage` types it must project onto.

/// Everything that can fail inside a Telegram service process.
///
/// The client-visible arm is deliberately absent at this milestone — see the module documentation.
/// [`TelegramError::Internal`] carries diagnostics that a response renderer cannot read: the
/// `source` is logged exactly once, at the boundary, and never serialized.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TelegramError {
    /// A failure inside this process. The subsystem is a telemetry attribute, never a
    /// client-visible fact; the source is logged once at the boundary, never rendered.
    #[error("internal failure in {subsystem}")]
    Internal {
        /// Which part of the process failed.
        subsystem: Subsystem,
        /// The diagnostics. Logged once at the boundary; never rendered into a response.
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

impl TelegramError {
    /// Constructs an internal failure from any error.
    pub fn internal(
        subsystem: Subsystem,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Internal {
            subsystem,
            source: Box::new(source),
        }
    }

    /// Writes the diagnostics exactly once, at the boundary, and nowhere else.
    pub fn log(&self) {
        match self {
            Self::Internal { subsystem, source } => {
                tracing::error!(subsystem = ?subsystem, chain = %source, "internal failure");
            }
        }
    }
}

/// Which part of the process failed. Bounded-cardinality telemetry only: never on a wire, never in
/// a response body, never in a metric label a request can influence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Subsystem {
    /// Reading or validating the typed configuration.
    Config,
    /// The subscriber, the exporter or an instrument.
    Telemetry,
    /// The HTTP harness: a listener, a middleware, or a handler.
    Http,
    /// The database pool, the schema, or a query.
    Persistence,
    /// The Telegram Bot API client boundary.
    BotApi,
}

impl core::fmt::Display for Subsystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Config => "config",
            Self::Telemetry => "telemetry",
            Self::Http => "http",
            Self::Persistence => "persistence",
            Self::BotApi => "bot_api",
        })
    }
}
