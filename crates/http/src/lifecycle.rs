//! The two facts readiness is computed from, and the checks it reports.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use metrics::gauge;
use telegram_core::RuntimeRole;
use telegram_telemetry::metrics::TELEGRAM_READINESS;

/// No database is configured for this role.
const DATABASE_ABSENT: u8 = 0;
/// The last probe answered.
const DATABASE_UP: u8 = 1;
/// The last probe did not answer.
const DATABASE_DOWN: u8 = 2;
/// No notification bus is configured for this role.
const NOTIFICATION_ABSENT: u8 = 0;
/// The fixed notification durable and consumer loop are healthy.
const NOTIFICATION_UP: u8 = 1;
/// The notification dependency is configured but unavailable or incompatible.
const NOTIFICATION_DOWN: u8 = 2;

/// The facts readiness is computed from.
///
/// Shared by the admin router, which reads it, and the shutdown sequence, which writes it.
#[derive(Debug)]
pub struct RuntimeState {
    /// The deployable this process is. Never read from the environment.
    role: RuntimeRole,
    /// Configuration validated, telemetry installed, every configured listener bound.
    startup_complete: AtomicBool,
    /// A shutdown signal arrived.
    draining: AtomicBool,
    /// The database: 0 not configured, 1 answering, 2 not answering. Three states rather than a
    /// `bool`, because "no database" and "a database that is down" must not report the same thing.
    database: AtomicU8,
    /// Notification bus: absent for webhook, up/down for dispatcher.
    notification_bus: AtomicU8,
}

impl RuntimeState {
    /// A process that has bound nothing yet: readiness fails, liveness does not.
    #[must_use]
    pub fn new(role: RuntimeRole) -> Self {
        let state = Self {
            role,
            startup_complete: AtomicBool::new(false),
            draining: AtomicBool::new(false),
            database: AtomicU8::new(DATABASE_ABSENT),
            notification_bus: AtomicU8::new(NOTIFICATION_ABSENT),
        };
        state.publish_readiness();
        state
    }

    /// The deployable this process is.
    #[must_use]
    pub fn role(&self) -> RuntimeRole {
        self.role
    }

    /// Record that this process has a configured database, before the first probe answers. Between
    /// this call and the first probe result the check reports failing with
    /// [`CheckReason::DependencyUnavailable`]: a dependency nobody has verified yet must not read
    /// as a passing one.
    pub fn mark_database_configured(&self) {
        self.database.store(DATABASE_DOWN, Ordering::Release);
        self.publish_readiness();
    }

    /// Every listener is bound and telemetry is up. Set exactly once.
    pub fn mark_startup_complete(&self) {
        self.startup_complete.store(true, Ordering::Release);
        self.publish_readiness();
    }

    /// Record what the latest database probe found.
    ///
    /// Called by the prober, not by a request: a readiness probe must not open a connection, or a
    /// saturated pool would make the health check the thing that finishes it off.
    pub fn set_database_reachable(&self, reachable: bool) {
        self.database.store(
            if reachable {
                DATABASE_UP
            } else {
                DATABASE_DOWN
            },
            Ordering::Release,
        );
        self.publish_readiness();
    }

    /// Record that the dispatcher has a configured notification dependency that has not yet been
    /// verified.
    pub fn mark_notification_configured(&self) {
        self.notification_bus
            .store(NOTIFICATION_DOWN, Ordering::Release);
        self.publish_readiness();
    }

    /// Record whether the fixed durable and its consumer loop are currently usable.
    pub fn set_notification_reachable(&self, reachable: bool) {
        self.notification_bus.store(
            if reachable {
                NOTIFICATION_UP
            } else {
                NOTIFICATION_DOWN
            },
            Ordering::Release,
        );
        self.publish_readiness();
    }

    /// A shutdown signal arrived. Readiness fails immediately; the listeners stay open.
    pub fn begin_draining(&self) {
        self.draining.store(true, Ordering::Release);
        self.publish_readiness();
    }

    /// Whether new work may be routed to this process.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.startup_complete.load(Ordering::Acquire)
            && !self.draining.load(Ordering::Acquire)
            && self.database.load(Ordering::Acquire) != DATABASE_DOWN
            && self.notification_bus.load(Ordering::Acquire) != NOTIFICATION_DOWN
    }

    /// The readiness checks, sorted by name.
    ///
    /// A `Vec`, never a map, so two consecutive probe bodies are byte-identical and `diff` is a
    /// usable tool at 03:00. There is deliberately no registry and no trait: a trait with one
    /// implementation is the abstraction this project rejects.
    #[must_use]
    pub fn checks(&self) -> Vec<Check> {
        let draining = self.draining.load(Ordering::Acquire);
        let started = self.startup_complete.load(Ordering::Acquire);
        let mut checks = vec![
            Check {
                name: CheckName::Database,
                state: CheckState::Pass,
                reason: None,
            },
            Check {
                name: CheckName::Drain,
                state: CheckState::from_pass(!draining),
                reason: draining.then_some(CheckReason::ShutdownRequested),
            },
            Check {
                name: CheckName::Startup,
                state: CheckState::from_pass(started),
                reason: (!started).then_some(CheckReason::StartupIncomplete),
            },
        ];

        // The database check exists only when a database does. Replace the placeholder's state with
        // what the latest probe found; when nothing is configured, drop it entirely.
        let placeholder = checks.remove(0);
        match self.database.load(Ordering::Acquire) {
            DATABASE_ABSENT => {}
            state => {
                let up = state == DATABASE_UP;
                checks.push(Check {
                    name: placeholder.name,
                    state: CheckState::from_pass(up),
                    reason: (!up).then_some(CheckReason::DependencyUnavailable),
                });
            }
        }

        if self.notification_bus.load(Ordering::Acquire) != NOTIFICATION_ABSENT {
            let up = self.notification_bus.load(Ordering::Acquire) == NOTIFICATION_UP;
            checks.push(Check {
                name: CheckName::NotificationBus,
                state: CheckState::from_pass(up),
                reason: (!up).then_some(CheckReason::DependencyUnavailable),
            });
        }

        // Alphabetical by name, so a probe body never depends on insertion order.
        checks.sort_unstable_by_key(|check| check.name);
        checks
    }

    /// `telegram_readiness{role}`, the aggregate of [`Self::checks`] that gate routing.
    fn publish_readiness(&self) {
        let value = if self.is_ready() { 1.0 } else { 0.0 };
        gauge!(TELEGRAM_READINESS, "role" => self.role.as_str()).set(value);
    }
}

/// One readiness check.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Check {
    /// The logical name of the subject.
    pub name: CheckName,
    /// Whether the subject passes.
    pub state: CheckState,
    /// Why it does not, when it does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<CheckReason>,
}

/// A logical token from a closed set. Never a hostname, port, DSN, latency or driver message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CheckName {
    /// The database answers. Present only when one is configured: a role with no database reports
    /// no database check rather than a passing one, because a passing check for something that does
    /// not exist is the readiness equivalent of an always-zero metric.
    Database,
    /// No shutdown signal has arrived.
    Drain,
    /// The exact pre-provisioned notification durable and consumer loop are usable.
    NotificationBus,
    /// Configuration, telemetry and every configured listener are up.
    Startup,
}

/// Whether one check passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// The subject is healthy.
    Pass,
    /// The subject is not healthy.
    Fail,
}

impl CheckState {
    /// The state a boolean subject is in.
    fn from_pass(pass: bool) -> Self {
        if pass { Self::Pass } else { Self::Fail }
    }
}

/// A closed set. NEVER a formatted dependency error: a driver message can carry a host, a port, a
/// user name and sometimes a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CheckReason {
    /// The process has not finished binding its listeners.
    StartupIncomplete,
    /// A shutdown signal arrived and this instance is draining.
    ShutdownRequested,
    /// The last probe of the database did not answer.
    DependencyUnavailable,
}
