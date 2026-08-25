//! Shared helpers for the dispatcher's integration test binaries: the injected clock and the
//! disposable-database constructor. Each binary includes this with `mod common;`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use ratatoskr_telegram_dispatcher::outbound::Clock;
use telegram_persistence::test_support::TestDatabase;

/// The injected time source: a test sets the second explicitly and advances it between calls;
/// queued jitters are handed out in order, defaulting to zero.
#[derive(Debug)]
pub(crate) struct FakeClock {
    current_secs: AtomicI64,
    next_jitters: Mutex<VecDeque<u64>>,
}

impl FakeClock {
    /// A clock frozen at `secs`.
    pub(crate) fn at(secs: i64) -> Arc<Self> {
        Arc::new(Self {
            current_secs: AtomicI64::new(secs),
            next_jitters: Mutex::new(VecDeque::new()),
        })
    }

    /// Move time forward by whole seconds.
    // Shared across test binaries; not every binary advances the clock, so the lint may
    // legitimately stay silent in some of them and `expect` would fire there instead.
    #[allow(dead_code)]
    pub(crate) fn advance_secs(&self, secs: i64) {
        self.current_secs.fetch_add(secs, Ordering::Relaxed);
    }

    /// The current injected second.
    pub(crate) fn now(&self) -> i64 {
        self.current_secs.load(Ordering::Relaxed)
    }
}

impl Clock for FakeClock {
    fn now_secs(&self) -> i64 {
        self.now()
    }

    fn jitter_millis(&self, _bound_ms: u64) -> u64 {
        self.next_jitters
            .lock()
            .expect("jitter queue")
            .pop_front()
            .unwrap_or(0)
    }
}

/// A disposable database per test.
pub(crate) async fn database() -> TestDatabase {
    TestDatabase::create()
        .await
        .expect("a disposable database must be creatable")
}
