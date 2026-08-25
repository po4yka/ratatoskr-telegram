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
pub(super) type BuildFuture = Pin<Box<dyn Future<Output = Result<Router, TelegramError>> + Send>>;

/// The boxed factory [`crate::start_public`] drives during startup.
pub(super) type PublicBuild = Box<dyn FnOnce(PublicContext) -> BuildFuture + Send>;

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

/// A boxed future for a background factory.
type BackgroundFuture = Pin<Box<dyn Future<Output = Result<(), TelegramError>> + Send>>;

/// The background-worker factory a role may bring: it spawns long-lived tasks during startup and
/// returns once they are running. Unlike a public router there is nothing to serve or drain — the
/// workers are detached and live until the process exits, exactly like the webhook's detached
/// intake worker. A failed factory is the standard startup failure: logged once at the boundary,
/// everything that opened gets closed, exit `1`.
pub struct Background {
    build: Option<Box<dyn Fn(PublicContext) -> BackgroundFuture + Send + Sync>>,
}

impl std::fmt::Debug for Background {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The factory is an opaque closure; whether one is present is the only honest fact.
        formatter
            .debug_struct("Background")
            .field("build", &self.build.is_some())
            .finish()
    }
}

impl Background {
    /// No background workers.
    #[must_use]
    pub fn none() -> Self {
        Self { build: None }
    }

    /// A factory that spawns this role's background workers during startup.
    #[must_use]
    pub fn new<F, Fut>(build: F) -> Self
    where
        F: Fn(PublicContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), TelegramError>> + Send + 'static,
    {
        Self {
            build: Some(Box::new(move |context| Box::pin(build(context)))),
        }
    }

    /// Whether a factory was supplied, so the lifecycle knows whether the step applies.
    #[must_use]
    pub(super) fn is_present(&self) -> bool {
        self.build.is_some()
    }

    /// Run the factory inside the caller's span. The context hands over clones of the shared
    /// configuration and database handle, so the lifecycle keeps its own.
    pub(super) fn call(&self, context: PublicContext) -> BackgroundFuture {
        match &self.build {
            Some(build) => build(context),
            None => Box::pin(std::future::ready(Ok(()))),
        }
    }
}
