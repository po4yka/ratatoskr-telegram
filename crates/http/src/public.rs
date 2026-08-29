//! The optional public listener a role may bring, and the seam that keeps one lifecycle for both.
//!
//! A role either serves updates or it does not. The dispatcher passes [`PublicRoutes::none`]; the
//! webhook passes a factory that receives the validated configuration and the prepared database
//! and returns a router — or a [`TelegramError`], which becomes the standard startup failure:
//! logged once at the boundary, everything that opened gets closed, exit `1`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Router;
use telegram_core::TelegramConfig;
use telegram_core::TelegramError;
use telegram_persistence::Database;
use tokio::sync::watch;
use tokio::task::JoinHandle;

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
    /// Shared operator-plane state for role-specific dependencies.
    pub runtime: Arc<crate::RuntimeState>,
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

/// The owned background tasks a role starts.
///
/// The shared lifecycle is the only consumer of this value. Cancellation stops new admission;
/// the owned handles prove every registered task has either completed or been aborted and reaped
/// before shared resources are closed.
pub struct BackgroundRuntime {
    cancel: watch::Sender<bool>,
    admission: Arc<tokio::sync::RwLock<()>>,
    admission_closed: Arc<AtomicBool>,
    tasks: Vec<JoinHandle<()>>,
    shutdown_started: bool,
}

impl BackgroundRuntime {
    /// Create one owned runtime around a supervisor task.
    #[must_use]
    pub fn new(cancel: watch::Sender<bool>, supervisor: JoinHandle<()>) -> Self {
        Self {
            cancel,
            admission: Arc::new(tokio::sync::RwLock::new(())),
            admission_closed: Arc::new(AtomicBool::new(false)),
            tasks: vec![supervisor],
            shutdown_started: false,
        }
    }

    /// Create one owned runtime from all of its direct worker handles.
    #[must_use]
    pub fn from_tasks(cancel: watch::Sender<bool>, tasks: Vec<JoinHandle<()>>) -> Self {
        Self {
            cancel,
            admission: Arc::new(tokio::sync::RwLock::new(())),
            admission_closed: Arc::new(AtomicBool::new(false)),
            tasks,
            shutdown_started: false,
        }
    }

    /// Start the admission-fenced cancellation request without waiting outside the grace budget.
    pub fn request_shutdown(&mut self) {
        if self.shutdown_started {
            return;
        }
        self.shutdown_started = true;
        self.admission_closed.store(true, Ordering::Release);
        self.cancel.send_replace(true);
        let admission = Arc::clone(&self.admission);
        self.tasks.push(tokio::spawn(async move {
            let _exclusive = admission.write().await;
        }));
    }

    /// Subscribe a newly registered worker to the same root cancellation state.
    #[must_use]
    pub fn cancel_receiver(&self) -> watch::Receiver<bool> {
        self.cancel.subscribe()
    }

    /// Fence the instant at which a worker may admit one new unit of work.
    #[must_use]
    pub fn admission_fence(&self) -> Arc<tokio::sync::RwLock<()>> {
        Arc::clone(&self.admission)
    }

    /// Synchronously sealed admission state checked inside each fenced worker section.
    #[must_use]
    pub fn admission_closed(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.admission_closed)
    }

    /// Number of directly owned workers, exposed for lifecycle composition tests.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Register one directly owned worker task.
    pub fn spawn<F>(&mut self, worker: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.push(tokio::spawn(worker));
    }

    /// Wait until every worker has completed after cancellation was requested.
    pub async fn join(mut self) {
        self.join_all().await;
    }

    pub(crate) fn abort_all(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }

    pub(crate) async fn join_all(&mut self) {
        for task in &mut self.tasks {
            let _ = task.await;
        }
    }
}

impl std::fmt::Debug for BackgroundRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackgroundRuntime")
            .field("tasks", &self.tasks.len())
            .finish_non_exhaustive()
    }
}

/// A boxed future for a background factory.
type BackgroundFuture =
    Pin<Box<dyn Future<Output = Result<BackgroundRuntime, TelegramError>> + Send>>;

/// The background-worker factory a role may bring. It returns every long-lived task to the shared
/// lifecycle; no worker is detached. A failed factory is the standard startup failure: logged once
/// at the boundary, everything that opened gets closed, exit `1`.
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

    /// A factory that assembles this role's owned background runtime during startup.
    #[must_use]
    pub fn new<F, Fut>(build: F) -> Self
    where
        F: Fn(PublicContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<BackgroundRuntime, TelegramError>> + Send + 'static,
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
        if let Some(build) = &self.build {
            build(context)
        } else {
            let (cancel, _cancelled) = watch::channel(false);
            Box::pin(std::future::ready(Ok(BackgroundRuntime::from_tasks(
                cancel,
                Vec::new(),
            ))))
        }
    }
}
