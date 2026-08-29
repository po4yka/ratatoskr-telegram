//! Ownership and bounded shutdown of role-specific background workers.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "assertions in a test binary"
)]

use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use telegram_core::RuntimeRole;
use telegram_core::config::ShutdownConfig;
use telegram_http::{BackgroundRuntime, RuntimeState, drain_and_close};
use tokio::sync::{oneshot, watch};

/// Sends exactly once when the task that owns this guard is actually dropped.
struct DropSignal(Option<oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(dropped) = self.0.take() {
            let _ = dropped.send(());
        }
    }
}

#[tokio::test]
async fn shutdown_signals_and_joins_background_before_returning() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Dispatcher));
    let config = ShutdownConfig {
        drain_seconds: 0,
        grace_seconds: 30,
    };
    let (shutdown_requested, mut shutdown) = watch::channel(false);
    let (cancelled, cancellation_observed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let supervisor = tokio::spawn(async move {
        shutdown
            .wait_for(|requested| *requested)
            .await
            .expect("the runtime owns the shutdown sender");
        cancelled
            .send(())
            .expect("the test is waiting for cancellation");
        released.await.expect("the test releases in-flight work");
    });
    let background = BackgroundRuntime::new(shutdown_requested, supervisor);

    let draining = tokio::spawn(async move {
        drain_and_close(&state, &config, Vec::new(), Some(background), pending()).await
    });

    cancellation_observed
        .await
        .expect("shutdown reaches the background supervisor");
    assert!(
        !draining.is_finished(),
        "shutdown must remain pending while admitted work is in flight"
    );

    release
        .send(())
        .expect("the background task is still being joined");
    let outcome = draining.await.expect("the shutdown task does not panic");
    assert!(outcome.graceful, "the admitted work completed in grace");
}

#[tokio::test(start_paused = true, flavor = "current_thread")]
async fn drain_deadline_aborts_and_awaits_stuck_background() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Dispatcher));
    let config = ShutdownConfig {
        drain_seconds: 0,
        grace_seconds: 1,
    };
    let (shutdown_requested, mut shutdown) = watch::channel(false);
    let (cancelled, cancellation_observed) = oneshot::channel();
    let (dropped, mut task_dropped) = oneshot::channel();
    let supervisor = tokio::spawn(async move {
        let _drop_signal = DropSignal(Some(dropped));
        shutdown
            .wait_for(|requested| *requested)
            .await
            .expect("the runtime owns the shutdown sender");
        cancelled
            .send(())
            .expect("the test is waiting for cancellation");
        pending::<()>().await;
    });
    let background = BackgroundRuntime::new(shutdown_requested, supervisor);

    let draining = tokio::spawn(async move {
        drain_and_close(&state, &config, Vec::new(), Some(background), pending()).await
    });

    cancellation_observed
        .await
        .expect("shutdown reaches the stuck supervisor");
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;

    let outcome = draining.await.expect("the shutdown task does not panic");
    assert!(!outcome.graceful, "the grace deadline forced cancellation");
    assert!(
        task_dropped.try_recv().is_ok(),
        "shutdown must await the aborted task before it returns"
    );
}

#[tokio::test]
async fn startup_rollback_aborts_and_awaits_an_already_bound_listener() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let served = telegram_http::serve(listener, axum::Router::new());
    let address = served.local_addr();

    served.abort_and_wait().await;

    assert!(
        tokio::net::TcpStream::connect(address).await.is_err(),
        "startup rollback must not leave the public listener detached"
    );
}

#[tokio::test(start_paused = true, flavor = "current_thread")]
async fn stalled_admission_fence_is_aborted_inside_the_grace_deadline() {
    let state = Arc::new(RuntimeState::new(RuntimeRole::Dispatcher));
    let config = ShutdownConfig {
        drain_seconds: 0,
        grace_seconds: 1,
    };
    let (cancel, _cancelled) = watch::channel(false);
    let mut background = BackgroundRuntime::from_tasks(cancel, Vec::new());
    let admission = background.admission_fence();
    let (started, holding_admission) = oneshot::channel();
    let (dropped, task_dropped) = oneshot::channel();
    background.spawn(async move {
        let _drop_signal = DropSignal(Some(dropped));
        let _admitted = admission.read().await;
        let _ = started.send(());
        pending::<()>().await;
    });
    holding_admission
        .await
        .expect("worker holds admission fence");

    let draining = tokio::spawn(async move {
        drain_and_close(&state, &config, Vec::new(), Some(background), pending()).await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;

    let outcome = draining.await.expect("bounded shutdown returns");
    assert!(!outcome.graceful);
    task_dropped
        .await
        .expect("the stalled admission worker is reaped before return");
}
