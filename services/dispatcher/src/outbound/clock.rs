//! The injected time source for every dispatcher timing decision.
//!
//! Whole Unix seconds are the unit on purpose: the persistence repositories take `i64` epoch
//! seconds for eligibility, leases, and render intervals, so one resolution everywhere keeps
//! scheduling arithmetic comparable without conversion layers. Jitter is the only sub-second
//! quantity, and it is a hint, not a clock reading.
//!
//! Tests never use [`SystemClock`]: they define their own fake implementing [`Clock`], which is
//! what makes backoff, refill, and interval behavior deterministic.

use std::time::{SystemTime, UNIX_EPOCH};

/// Where the dispatcher reads time. Object-safe and shared across tasks, so the sender loop, the
/// limiter, and the consumer can all hold `Arc<dyn Clock>` or borrow one.
pub trait Clock: Send + Sync {
    /// The current instant as whole seconds since the Unix epoch — the same form every
    /// persistence timestamp argument takes.
    fn now_secs(&self) -> i64;

    /// A pseudo-random delay in `[0, bound_ms)` milliseconds, used to spread retry storms.
    ///
    /// Implementations may be cheap and low-quality (a nanosecond remainder suffices): jitter
    /// only needs to decorrelate callers, not to be unpredictable against an adversary.
    fn jitter_millis(&self, bound_ms: u64) -> u64;
}

/// The production clock: wall-clock seconds, jitter from sub-second nanos.
///
/// The jitter entropy source is deliberately unambitious — the nanosecond field of
/// [`SystemTime::now`] — because the only requirement is that two senders retrying after the same
/// failure do not pick the same millisecond. No `rand` dependency is pulled in for that.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> i64 {
        let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH);
        // A clock set before the epoch has no meaningful second count; 0 degrades scheduling,
        // never correctness, and cannot panic the sender loop.
        since_epoch.map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
    }

    fn jitter_millis(&self, bound_ms: u64) -> u64 {
        if bound_ms == 0 {
            return 0;
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| u64::from(duration.subsec_nanos()));
        nanos % bound_ms
    }
}
