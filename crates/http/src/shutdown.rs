//! Signal, drain, close, flush — the part everyone gets wrong, spelled out.

use std::future::{Future, IntoFuture as _};
use std::time::Duration;

use axum::Router;
use telegram_core::config::ShutdownConfig;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::Instrument as _;
use tracing::field::Empty;

use crate::lifecycle::RuntimeState;
use crate::public::BackgroundRuntime;

/// One listener being served, and the trigger that stops it accepting.
#[derive(Debug)]
pub struct Served {
    /// The address this listener answers on, for startup records.
    local_addr: std::net::SocketAddr,
    /// Resolves the server's graceful-shutdown future.
    close: oneshot::Sender<()>,
    /// Completes when every in-flight request on this listener has finished.
    task: JoinHandle<std::io::Result<()>>,
}

impl Served {
    /// The address the listener accepted connections on.
    #[must_use]
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    /// Abort this listener and reap its task during startup rollback.
    pub async fn abort_and_wait(self) {
        let Self { close, task, .. } = self;
        drop(close);
        task.abort();
        let _ = task.await;
    }
}

/// Serves `router` on `listener` until [`drain_and_close`] closes it.
///
/// # Panics
///
/// Never itself; the spawned task panics only if axum's accept loop does.
#[must_use]
pub fn serve(listener: TcpListener, router: Router) -> Served {
    let (close, closed) = oneshot::channel();
    // A bound listener always knows its address; the fallback only keeps this total for a caller
    // that somehow passed an unbound one, and is never rendered as meaningful.
    let local_addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(_) => std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
    };
    let task = tokio::spawn(
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                // A dropped sender means the process is going down anyway.
                let _ = closed.await;
            })
            .into_future(),
    );
    Served {
        local_addr,
        close,
        task,
    }
}

/// What the shutdown sequence did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShutdownOutcome {
    /// Whether every in-flight request finished inside the grace window.
    pub graceful: bool,
    /// Whether a second signal short-circuited the sequence.
    pub interrupted: bool,
}

/// Steps 1–6 of the shutdown sequence. Every step exists because skipping it produces a specific
/// observed failure.
///
/// 1. A signal arrived. Open `telegram.shutdown`; log at INFO.
/// 2. [`RuntimeState::begin_draining`]. **Readiness returns 503 immediately. The listeners stay
///    open.** `telegram_readiness` drops to 0.
/// 3. Sleep `drain_seconds`. Existing and brand-new requests still succeed. This is the window in
///    which whatever routes here stops routing; skipping it is the direct cause of failed requests
///    on every deploy.
/// 4. Every server completes its graceful shutdown: stop accepting, let in-flight requests finish,
///    bounded by `grace_seconds`.
/// 5. If the grace window expires, log WARN and continue anyway. A deploy is never blocked by one
///    stuck request.
/// 6. The caller flushes telemetry and exits 0.
///
/// `interrupt` is the second signal: when it resolves first, the sequence skips straight to step 6.
/// `/health/live` answers 200 throughout.
pub async fn drain_and_close(
    state: &RuntimeState,
    config: &ShutdownConfig,
    servers: Vec<Served>,
    mut background: Option<BackgroundRuntime>,
    interrupt: impl Future<Output = ()> + Send,
) -> ShutdownOutcome {
    let span = tracing::info_span!(
        "telegram.shutdown",
        role = state.role().as_str(),
        drain_seconds = config.drain_seconds,
        graceful = Empty,
    );

    async move {
        tracing::info!("a shutdown signal arrived; draining");
        state.begin_draining();
        if let Some(background) = background.as_mut() {
            background.request_shutdown();
        }

        tokio::pin!(interrupt);
        let mut interrupted = tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(config.drain_seconds)) => false,
            () = &mut interrupt => true,
        };

        let mut tasks = Vec::with_capacity(servers.len());
        for served in servers {
            // A closed receiver means that server already stopped.
            let _ = served.close.send(());
            tasks.push(served.task);
        }

        let graceful = if interrupted {
            false
        } else {
            // The grace window is part of the sequence, so a second signal short-circuits it too.
            // Without the second arm a second Ctrl-C during step 4 is ignored and the operator
            // waits out the whole `grace_seconds`.
            tokio::select! {
                closed = tokio::time::timeout(
                    Duration::from_secs(config.grace_seconds),
                    wait_for_all(&mut tasks, background.as_mut()),
                ) => closed.is_ok(),
                () = &mut interrupt => {
                    interrupted = true;
                    false
                }
            }
        };
        if !graceful {
            abort_all(&tasks);
            if let Some(background) = background.as_mut() {
                background.abort_all();
            }
            wait_for_all(&mut tasks, background.as_mut()).await;
            if interrupted {
                tracing::warn!(
                    "a second signal arrived; closing without waiting for in-flight work"
                );
            } else {
                tracing::warn!(
                    "the grace window expired with work still in flight; aborted every owned task"
                );
            }
        }

        tracing::Span::current().record("graceful", graceful);
        tracing::info!(graceful, interrupted, "shutdown complete");

        ShutdownOutcome {
            graceful,
            interrupted,
        }
    }
    .instrument(span)
    .await
}

/// Stops every server task without waiting for its in-flight work. Only ever reached from the
/// second-signal path, where an operator has said twice that they want the process gone.
fn abort_all(tasks: &[JoinHandle<std::io::Result<()>>]) {
    for task in tasks {
        task.abort();
    }
}

/// Completes when every server task has finished. Borrows rather than consumes, so the tasks are
/// still there to abort if a second signal cancels this future.
async fn wait_for_all(
    tasks: &mut [JoinHandle<std::io::Result<()>>],
    background: Option<&mut BackgroundRuntime>,
) {
    for task in tasks {
        // A server task that failed has already logged; shutdown continues.
        let _ = task.await;
    }
    if let Some(background) = background {
        background.join_all().await;
    }
}

/// Stop startup-created tasks when a later startup step fails.
pub(crate) async fn abort_background(mut background: Option<BackgroundRuntime>) {
    if let Some(background) = background.as_mut() {
        background.request_shutdown();
        background.abort_all();
        background.join_all().await;
    }
}

pub(crate) async fn abort_served(served: Option<Served>) {
    if let Some(served) = served {
        served.abort_and_wait().await;
    }
}

/// Resolves on the first SIGTERM or SIGINT.
///
/// A process that cannot register a handler waits forever rather than exiting: an unkillable pod is
/// visible to an operator, a pod that exits at startup for an unrelated reason is not.
pub(crate) async fn signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = terminate.recv() => {},
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result {
                            tracing::error!(%error, "the interrupt handler could not be installed");
                        }
                    },
                }
            }
            Err(error) => {
                tracing::error!(%error, "the termination handler could not be installed");
                std::future::pending::<()>().await;
            }
        }
    }

    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "the interrupt handler could not be installed");
        std::future::pending::<()>().await;
    }
}
