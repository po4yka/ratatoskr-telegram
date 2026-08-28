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
mod public;
mod shutdown;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use telegram_core::TelegramError;
use tokio::net::TcpListener;
use tracing::field::Empty;

pub use crate::admin::admin_router;
pub use crate::lifecycle::{Check, CheckName, CheckReason, CheckState, RuntimeState};
pub use crate::public::{Background, PublicContext, PublicRoutes};
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
///    Both roles write through the pool (the webhook admits updates, the dispatcher delivers), so
///    both REFUSE to start when that fails.
/// 5. Build and bind the public listener through `public_routes`, when the role brings one. A failed
///    build or bind logs at ERROR, runs the standard cleanup, and exits `1`.
/// 6. Bind the admin listener. On failure log at ERROR and exit `1`.
/// 7. [`RuntimeState::mark_startup_complete`] — readiness flips to 200.
/// 8. Serve until SIGTERM or SIGINT, then [`drain_and_close`].
/// 9. Stop the prober, close the pool, `TelemetryGuard::shutdown()`; exit `0`.
pub async fn run(role: telegram_core::RuntimeRole, public_routes: PublicRoutes) -> ExitCode {
    run_with_background(role, public_routes, Background::none()).await
}

/// The full lifecycle with one addition over [`run`]: a background factory that spawns this
/// role's long-lived workers after the configuration is validated and the database is prepared,
/// and before any listener binds. A failed factory is the standard startup failure — logged once
/// inside the startup span, everything closed, exit `1`. Roles without background workers pass
/// [`Background::none`], which makes this byte-identical to [`run`].
///
/// # Errors
///
/// Never returns an error; failures are the documented exit codes.
pub async fn run_with_background(
    role: telegram_core::RuntimeRole,
    public_routes: PublicRoutes,
    background: Background,
) -> ExitCode {
    let config = match telegram_core::config::load(role) {
        Ok(config) => Arc::new(config),
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
    announce(&config);

    let runtime = Arc::new(RuntimeState::new(role));

    let database = prepare_database(role, &config, &runtime).await;
    if database.is_none() && role_requires_database(role) {
        tracing::error!(
            "the configured database could not be prepared; {} writes update deduplication \
             through it and cannot serve updates without it",
            role.binary_name(),
        );
        guard.shutdown();
        return ExitCode::FAILURE;
    }

    // The role's background workers start here, on prepared ground: their factory sees the same
    // context a public-router factory would, and its failure takes the identical cleanup path.
    if background.is_present() {
        let context = PublicContext {
            config: Arc::clone(&config),
            database: database.clone(),
            runtime: Arc::clone(&runtime),
        };
        if let Err(error) = start_background(&background, context, &startup).await {
            error.log();
            close_database(database.as_ref()).await;
            guard.shutdown();
            return ExitCode::FAILURE;
        }
    }

    // The public listener comes from the role. Its factory runs inside the startup span so its
    // failures carry the same fields as every other startup record.
    let public = match public_routes.take() {
        None => None,
        Some(build) => {
            let context = PublicContext {
                config: Arc::clone(&config),
                database: database.clone(),
                runtime: Arc::clone(&runtime),
            };
            match start_public(build, context, &startup, &config).await {
                Ok(served) => served,
                Err(error) => {
                    error.log();
                    close_database(database.as_ref()).await;
                    guard.shutdown();
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    let Some(admin) = startup
        .in_scope(|| bind_admin(&config, Arc::clone(&runtime), guard.metrics_handle()))
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
    record_startup_complete(&startup, &config, public.as_ref(), database.is_some());
    drop(startup);

    serve_until_signal(
        &runtime,
        &config.shutdown,
        admin,
        public,
        prober,
        database.as_ref(),
        guard,
    )
    .await
}

/// The one INFO line that names every listener and dependency the process started with.
fn record_startup_complete(
    startup: &tracing::Span,
    config: &telegram_core::TelegramConfig,
    public: Option<&Served>,
    database_present: bool,
) {
    startup.in_scope(|| {
        tracing::info!(
            admin = %config.admin.bind,
            public = public.map(shutdown::Served::local_addr).map_or(
                "none".to_owned(),
                |addr| addr.to_string(),
            ),
            database = database_present,
            "startup complete",
        );
    });
}

/// Serve until SIGTERM or SIGINT, then run the documented teardown in its documented order:
/// drain listeners, stop the prober, close the pool after the grace window, shut telemetry down.
async fn serve_until_signal(
    runtime: &Arc<RuntimeState>,
    shutdown_config: &telegram_core::config::ShutdownConfig,
    admin: Served,
    public: Option<Served>,
    prober: Option<tokio::task::JoinHandle<()>>,
    database: Option<&telegram_persistence::Database>,
    guard: telegram_telemetry::TelemetryGuard,
) -> ExitCode {
    shutdown::signal().await;
    let mut servers = vec![admin];
    if let Some(public) = public {
        servers.push(public);
    }
    drain_and_close(runtime, shutdown_config, servers, shutdown::signal()).await;

    if let Some(prober) = prober {
        prober.abort();
    }
    // After the listener stopped accepting and the grace window closed, so an in-flight request
    // kept its connection for its whole life.
    close_database(database).await;
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

/// Validate configuration, connect to the owned database, and apply (webhook) or verify
/// (dispatcher) the one current schema without binding listeners.
pub async fn check_schema(role: telegram_core::RuntimeRole) -> ExitCode {
    let config = match telegram_core::config::load(role) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{}", error.report(role));
            return ExitCode::from(error.exit_code());
        }
    };
    let Some(database_config) = config.database.as_ref() else {
        eprintln!("{}: database configuration is absent", role.binary_name());
        return ExitCode::from(78);
    };
    let database = match telegram_persistence::Database::connect(database_config).await {
        Ok(database) => database,
        Err(error) => {
            eprintln!(
                "{}: database connection failed: {error}",
                role.binary_name()
            );
            return ExitCode::FAILURE;
        }
    };
    let result = match role {
        telegram_core::RuntimeRole::Webhook => database.apply_schema().await,
        telegram_core::RuntimeRole::Dispatcher => database.verify_schema().await,
    };
    database.close().await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: schema check failed: {error}", role.binary_name());
            ExitCode::FAILURE
        }
    }
}

/// The single INFO line that says what the process actually believes, and the one non-fatal
/// warning. Safe by type: `SecretString` has no `Display` and renders as `[REDACTED]`.
///
/// Start the role's background workers inside the startup span. The factory's error propagates;
/// the caller owns the standard cleanup.
async fn start_background(
    background: &Background,
    context: PublicContext,
    startup: &tracing::Span,
) -> Result<(), TelegramError> {
    startup.in_scope(|| background.call(context)).await
}

/// Build and bind the public listener inside the startup span. The factory's or bind's error
/// propagates; the caller owns the standard cleanup.
async fn start_public(
    build: crate::public::PublicBuild,
    context: PublicContext,
    startup: &tracing::Span,
    config: &telegram_core::TelegramConfig,
) -> Result<Option<Served>, TelegramError> {
    let router = startup.in_scope(|| build(context)).await?;
    Ok(startup.in_scope(|| bind_public(config, router)).await)
}

/// Close the pool behind an optional database handle, once its work is done or abandoned.
async fn close_database(database: Option<&telegram_persistence::Database>) {
    if let Some(database) = database {
        database.close().await;
    }
}

fn announce(config: &telegram_core::TelegramConfig) {
    tracing::info!(
        config = ?config,
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
/// Absent configuration is NOT itself a failure — for a role with no requirement. A PRESENT
/// configuration that cannot be reached or prepared returns `None` and the caller decides: the
/// webhook writes through the pool and refuses to start ([`role_requires_database`]); the
/// dispatcher, with no such route yet, degrades to a failing readiness check while staying up.
async fn prepare_database(
    role: telegram_core::RuntimeRole,
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

    let schema = match role {
        telegram_core::RuntimeRole::Webhook => database.apply_schema().await,
        telegram_core::RuntimeRole::Dispatcher => database.verify_schema().await,
    };
    if let Err(error) = schema {
        tracing::warn!(error = %error, "the current schema could not be prepared");
        return None;
    }

    // The first probe happens BEFORE any listener opens, so the process never reports itself ready
    // with an unverified dependency.
    runtime.set_database_reachable(database.ping().await.is_ok());

    Some(database)
}

/// Whether this role's routes or workers write through the database, so an unreachable one is a
/// startup refusal rather than a degraded-but-up state. Both roles refuse since item 4: the
/// webhook admits updates through the pool, and the dispatcher claims and settles every Bot API
/// write through it.
const fn role_requires_database(role: telegram_core::RuntimeRole) -> bool {
    matches!(
        role,
        telegram_core::RuntimeRole::Webhook | telegram_core::RuntimeRole::Dispatcher
    )
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

/// Bind the public listener the role built, when it built one.
///
/// `None` on failure; the caller exits `1` through the same path as an admin bind failure.
async fn bind_public(
    config: &telegram_core::TelegramConfig,
    router: axum::Router,
) -> Option<Served> {
    let Some(webhook) = config.webhook.as_ref() else {
        // A factory without webhook configuration cannot have produced a router that needs one —
        // and validation refuses the combination anyway. Unreachable, but total.
        return None;
    };
    match TcpListener::bind(webhook.bind).await {
        Ok(listener) => Some(serve(listener, router)),
        Err(error) => {
            tracing::error!(bind = %webhook.bind, %error, "the public listener could not bind");
            None
        }
    }
}

/// Milliseconds with one decimal, as the startup span records it.
fn duration_ms(elapsed: Duration) -> f64 {
    (elapsed.as_secs_f64() * 1000.0 * 10.0).round() / 10.0
}
