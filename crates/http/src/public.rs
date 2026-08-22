//! The optional public listener a role may bring, and the seam that keeps one lifecycle for both.
//!
//! A role either serves updates or it does not. The dispatcher passes [`PublicRoutes::none`]; the
//! webhook passes a factory that receives the validated configuration and the prepared database
//! and returns a router — or a [`TelegramError`], which becomes the standard startup failure:
//! logged once at the boundary, everything that opened gets closed, exit `1`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::Router;
use telegram_core::TelegramConfig;
use telegram_core::TelegramError;
use telegram_persistence::Database;

/// A boxed future so the trait-object seam stays `Send` end to end.
type BuildFuture = Pin<Box<dyn Future<Output = Result<Router, TelegramError>> + Send>>;

/// What a route factory receives: the whole validated configuration and, when one was configured
/// and reached, the database handle. The factory owns every further decision.
#[derive(Debug)]
pub struct PublicContext {
    /// The validated configuration. Shared, because the lifecycle keeps reading it after the
    /// factory returns.
    pub config: Arc<TelegramConfig>,
    /// The prepared database, when one is configured and reachable. A role whose factory needs one
    /// has already refused to start without it.
    pub database: Option<Database>,
}

/// The public-router factory, or the deliberate absence of one.
pub struct PublicRoutes {
    build: Option<Box<dyn FnOnce(PublicContext) -> BuildFuture + Send>>,
}

impl std::fmt::Debug for PublicRoutes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The factory is an opaque closure; whether one is present is the only honest fact.
        formatter
            .debug_struct("PublicRoutes")
            .field("build", &self.build.is_some())
            .finish()
    }
}

impl PublicRoutes {
    /// No public listener. The dispatcher's answer until its own plan item needs one.
    #[must_use]
    pub fn none() -> Self {
        Self { build: None }
    }

    /// A factory that builds the public router during startup.
    #[must_use]
    pub fn new<F, Fut>(build: F) -> Self
    where
        F: FnOnce(PublicContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Router, TelegramError>> + Send + 'static,
    {
        Self {
            build: Some(Box::new(move |context| Box::pin(build(context)))),
        }
    }

    /// Consumes the factory, if there is one. The lifecycle calls it exactly once, at the point
    /// where a failed build can still be cleaned up by the standard path.
    #[must_use]
    pub(super) fn take(self) -> Option<Box<dyn FnOnce(PublicContext) -> BuildFuture + Send>> {
        self.build
    }
}
