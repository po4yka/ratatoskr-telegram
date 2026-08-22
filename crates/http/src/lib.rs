//! The shared HTTP harness every deployable runs: `run(role)` and nothing else in a `main`.
//!
//! - [`run`] and [`check_config`] — the whole process lifecycle, so the two binaries cannot drift.
//! - [`admin_router`] — liveness, readiness, metrics and version, on the operator listener only.
//! - [`RuntimeState`] — the facts readiness is computed from.
//! - [`serve`] and [`drain_and_close`] — the drain-then-close-then-flush sequence.
//!
//! # The one documented exception
//!
//! The admin listener carries no contract error envelope: `/health/ready` returning 503 must tell
//! an operator WHICH check failed. The public surface that will carry envelopes arrives with the
//! webhook listener.
//!
//! # Exit codes
//!
//! | Code | Meaning |
//! |---|---|
//! | `0` | Clean start and clean shutdown |
//! | `1` | Runtime startup failure: telemetry initialisation, or a listener that could not bind |
//! | `78` | `EX_CONFIG` — the configuration is unreadable or invalid; nothing was bound |

mod admin;
mod lifecycle;
mod shutdown;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::TcpListener;
use tracing::field::Empty;

pub use crate::admin::admin_router;
pub use crate::lifecycle::{Check, CheckName, CheckReason, CheckState, RuntimeState};
pub use crate::shutdown::{Served, ShutdownOutcome, drain_and_close, serve};

/// How often the database prober asks whether the dependency is still there.
///
/// Five seconds: long enough that the probe is not itself load, short enough that a readiness state
/// is never more than one scrape interval stale.
const DATABASE_PROBE_INTERVAL: Duration = Duration::from_secs(5);

/// The whole process lifecycle for one runtime role. Each binary's `main` is this call and nothing
/// else.
///
/// Sequence, in this order and no other:
///
/// 1. `telegram_core::config::load(role)` — on failure write `ConfigError::report` to stderr and
///    exit `78`.
/// 2. `telegram_telemetry::init` — on failure write to stderr, exit `1`. Telemetry is initialised
///    AFTER validation so an invalid `log_filter` is a configuration error, not a failure inside
///    subscriber setup where nothing can report it.
/// 3. Open `telegram.startup`. Log the effective configuration at INFO (safe by type).
///    `telegram_build_info` is set by `telegram_telemetry::init`.
/// 4. Connect the configured database and apply the embedded schema — all BEFORE any listener
///    binds, so a process never reports itself ready with an unverified or unprepared dependency.
///    A configured database that cannot be reached is a WARN and a failing readiness check, not an
///    abort: see `prepare_database`.
/// 5. Bind the admin listener. On failure log at ERROR and exit `1`.
/// 6. [`RuntimeState::mark_startup_complete`] — readiness flips to 200.
/// 7. Serve until SIGTERM or SIGINT, then [`drain_and_close`].
/// 8. Stop the prober, close the pool, `TelemetryGuard::shutdown()`; exit `0`.
pub async fn run(role: telegram_core::RuntimeRole) -> ExitCode {
    let config = match telegram_core::config::load(role) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{}", error.report(role));
            return ExitCode::from(error.exit_code());
        }
    };

    let guard = match telegram_telemetry::init(&config.telemetry, role) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!(
                "{}: refusing to start; telemetry could not be initialised: {error}",
                role.binary_name()
            );
            return ExitCode::FAILURE;
        }
    };

    let started = Instant::now();
    let startup = tracing::info_span!(
        "telegram.startup",
        role = role.as_str(),
        version = telegram_telemetry::identity::VERSION,
        git_sha = telegram_telemetry::identity::GIT_SHA,
        duration_ms = Empty,
    );
    announce(role, &config);

    let runtime = Arc::new(RuntimeState::new(role));
    let metrics_handle = guard.metrics_handle();

    let database = prepare_database(&config, &runtime).await;

    let Some(admin) = startup
        .in_scope(|| bind_admin(&config, Arc::clone(&runtime), metrics_handle))
        .await
    else {
        if let Some(database) = database.as_ref() {
            database.close().await;
        }
        guard.shutdown();
        return ExitCode::FAILURE;
    };

    let prober = database
        .as_ref()
        .map(|database| spawn_database_prober(Arc::new(database.clone()), Arc::clone(&runtime)));

    runtime.mark_startup_complete();
    startup.record("duration_ms", duration_ms(started.elapsed()));
    startup.in_scope(|| {
        tracing::info!(
            admin = %config.admin.bind,
            database = database.is_some(),
            "startup complete",
        );
    });
    drop(startup);

    shutdown::signal().await;
    drain_and_close(&runtime, &config.shutdown, vec![admin], shutdown::signal()).await;

    if let Some(prober) = prober {
        prober.abort();
    }
    if let Some(database) = database.as_ref() {
        // After the listener stopped accepting and the grace window closed, so an in-flight request
        // kept its connection for its whole life.
        database.close().await;
    }

    guard.shutdown();
    ExitCode::SUCCESS
}

/// `<binary> check-config`: load and validate without binding anything; write the effective
/// configuration or the failure report; exit `0` or `78`.
///
/// It exists so an environment can be validated in CI or an init container before a process is
/// allowed to start. Both outputs go to stderr: no subscriber exists yet, and the workspace forbids
/// writing to stdout so that a stray line can never be mistaken for a log record.
#[must_use]
pub fn check_config(role: telegram_core::RuntimeRole) -> ExitCode {
    match telegram_core::config::load(role) {
        Ok(config) => {
            // Safe by type: the secret members render as `[REDACTED]` however deeply nested.
            eprintln!(
                "{}: configuration is valid.\n{config:#?}",
                role.binary_name()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", error.report(role));
            ExitCode::from(error.exit_code())
        }
    }
}

/// The single INFO line that says what the process actually believes, and the one non-fatal
/// warning. Safe by type: `SecretString` has no `Display` and renders as `[REDACTED]`.
fn announce(role: telegram_core::RuntimeRole, config: &telegram_core::TelegramConfig) {
    tracing::info!(
        config = ?config,
        role = %role,
        version = telegram_telemetry::identity::VERSION,
        git_sha = telegram_telemetry::identity::GIT_SHA,
        "effective configuration",
    );
    if !config.admin.bind.ip().is_loopback() {
        tracing::warn!(
            bind = %config.admin.bind,
            "the admin plane is not bound to a loopback address; it must never be published \
             through an ingress",
        );
    }
    if config.telemetry.otlp.is_none() {
        tracing::warn!(
            "no OTLP endpoint is configured; spans are created and carry real trace ids, but \
             nothing is exported",
        );
    }
}

/// Connect and prepare the configured database, if there is one.
///
/// Absent configuration is NOT a failure — no request path reads the database yet, and a role
/// without one reports no database check rather than a failing one. A PRESENT configuration that
/// cannot be reached or prepared is also not a startup abort at this milestone: no route consumes
/// the database, so refusing to start would take down a process that could still serve its operator
/// plane truthfully reporting `dependency_unavailable`. Readiness stays 503 and the reason is
/// logged; when the first feature that writes through the pool arrives, this branch becomes a
/// refusal, exactly as the sibling services treat a dependency their routes cannot serve without.
async fn prepare_database(
    config: &telegram_core::TelegramConfig,
    runtime: &Arc<RuntimeState>,
) -> Option<telegram_persistence::Database> {
    let database_config = config.database.as_ref()?;

    // Configured, therefore checked — and not passing until a probe says so. A dependency nobody
    // has verified must not read as a passing one.
    runtime.mark_database_configured();

    let database = match telegram_persistence::Database::connect(database_config).await {
        Ok(database) => database,
        Err(error) => {
            // Safe fields only: the class of failure, never the URL.
            tracing::warn!(error = %error, "the configured database could not be reached");
            return None;
        }
    };

    if let Err(error) = database.apply_schema().await {
        tracing::warn!(error = %error, "the schema could not be applied");
        return None;
    }

    // The first probe happens BEFORE the listener opens, so the process never reports itself ready
    // with an unverified dependency.
    runtime.set_database_reachable(database.ping().await.is_ok());

    Some(database)
}

/// Probe the database on a fixed interval until the task is aborted.
fn spawn_database_prober(
    database: Arc<telegram_persistence::Database>,
    runtime: Arc<RuntimeState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(DATABASE_PROBE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; the pre-bind probe already covered it.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            runtime.set_database_reachable(database.ping().await.is_ok());
        }
    })
}

/// Bind the admin listener.
///
/// `None` on failure; the caller exits `1`. The error is logged here, inside the startup span, so
/// it carries the same fields as every other startup record.
async fn bind_admin(
    config: &telegram_core::TelegramConfig,
    runtime: Arc<RuntimeState>,
    metrics: metrics_exporter_prometheus::PrometheusHandle,
) -> Option<Served> {
    match TcpListener::bind(config.admin.bind).await {
        Ok(listener) => {
            let render = move || metrics.render();
            Some(serve(listener, admin_router(runtime, render)))
        }
        Err(error) => {
            tracing::error!(bind = %config.admin.bind, %error, "the admin listener could not bind");
            None
        }
    }
}

/// Milliseconds with one decimal, as the startup span records it.
fn duration_ms(elapsed: Duration) -> f64 {
    (elapsed.as_secs_f64() * 1000.0 * 10.0).round() / 10.0
}
