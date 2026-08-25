//! The outbound rate gate: one global token budget plus a per-chat minimum spacing, both read
//! from the injected [`Clock`] and nothing else.
//!
//! WHY a token bucket with burst capacity exactly ONE token: Telegram's documented sustained
//! ceiling is roughly 30 messages per second globally for one bot, and bursts above the sustained
//! rate are precisely what turn into `429`s the sender then has to honor. The bucket therefore
//! never banks more than one immediate send and refills continuously at the configured budget.
//! WHY a per-chat gate on top: Telegram also paces individual chats, and spacing same-chat sends
//! is what keeps progress-edit coalescing from becoming its own burst.
//!
//! The chat gate is evaluated BEFORE the global gate: when both would deny, the chat answer is
//! the more actionable hint (the sender can go work another chat), and the minimum-gap contract
//! below pins that ordering. A token is consumed only when BOTH gates pass — a denied call never
//! burns budget for nothing.
//!
//! All timing reads go through the injected clock; the limiter holds no wall clock of its own,
//! so tests drive it deterministically. The clock's resolution is whole seconds (it feeds the
//! persistence API's epoch-second inputs), so refill and gap arithmetic quantize to second ticks;
//! millisecond figures remain exact as WAIT HINTS computed from those ticks. Interior state sits
//! behind a std `Mutex`: this is a synchronous, sub-microsecond critical path called from the
//! sender loop, where an async lock would only add scheduling cost.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::outbound::clock::Clock;

/// One whole token expressed in milli-tokens. The bucket runs in integer milli-tokens so refill
/// and consumption are exact under the seconds-resolution clock — no floating-point drift can
/// ever mint an extra send or swallow a due one.
const ONE_TOKEN_MILLI: u64 = 1000;

/// What the sender must do with one candidate delivery right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDecision {
    /// Both gates passed: the call may go on the wire.
    Proceed,
    /// The global budget is spent; retry after roughly `after_ms`.
    GlobalWait {
        /// Milliseconds until the next global token accrues.
        after_ms: u64,
    },
    /// This chat sent too recently; retry after roughly `after_ms`.
    ChatWait {
        /// Milliseconds until the chat's minimum interval has fully elapsed.
        after_ms: u64,
    },
}

/// The mutable side of the limiter: bucket balance in milli-tokens, last refill tick, and each
/// chat's last proceed instant.
///
/// The map grows with distinct chats seen and is never pruned; deployments are owner-first with a
/// handful of chats, so the entries are bounded by deployment shape rather than by traffic.
#[derive(Debug, Default)]
struct LimiterState {
    tokens_milli: u64,
    last_refill_secs: Option<i64>,
    last_proceed_by_chat: HashMap<i64, i64>,
}

/// The two-gate rate limiter shared by every sender task.
#[derive(Debug)]
pub struct DeliveryLimiter {
    global_per_second: u32,
    per_chat_min_interval_ms: u64,
    state: Mutex<LimiterState>,
}

impl DeliveryLimiter {
    /// Build a limiter with `global_per_second` tokens of sustained budget and a
    /// `per_chat_min_interval_ms` minimum gap between consecutive sends to one chat.
    ///
    /// The bucket starts full (one burst token) so the first send after startup never waits on
    /// bookkeeping that has not happened yet.
    #[must_use]
    pub fn new(global_per_second: u32, per_chat_min_interval_ms: u64) -> Self {
        Self {
            global_per_second,
            per_chat_min_interval_ms,
            state: Mutex::new(LimiterState {
                tokens_milli: ONE_TOKEN_MILLI,
                ..LimiterState::default()
            }),
        }
    }

    /// Ask whether a delivery to `chat_id` may go on the wire now, according to `clock`.
    ///
    /// On [`RateDecision::Proceed`] one global token is consumed and the chat's last-proceed
    /// instant moves to the current tick; every denial leaves the state untouched apart from
    /// refill accrual. A poisoned lock is recovered rather than propagated: the guarded
    /// arithmetic cannot leave the state half-valid, and wedging the sender loop over a panic
    /// that already happened would turn one bad delivery into a stalled dispatcher.
    #[must_use]
    pub fn try_acquire(&self, clock: &dyn Clock, chat_id: i64) -> RateDecision {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = clock.now_secs();
        Self::refill(&mut state, now, u64::from(self.global_per_second));

        // Chat gate first: when both gates would deny, the chat answer is the more actionable
        // hint, and the minimum-gap contract pins this ordering.
        if let Some(denial) = Self::chat_denial(&state, chat_id, now, self.per_chat_min_interval_ms)
        {
            return denial;
        }

        if state.tokens_milli < ONE_TOKEN_MILLI {
            return RateDecision::GlobalWait {
                after_ms: millis_for_one_token(self.global_per_second),
            };
        }

        state.tokens_milli -= ONE_TOKEN_MILLI;
        state.last_proceed_by_chat.insert(chat_id, now);
        RateDecision::Proceed
    }

    /// Accrue budget for every fully elapsed second, capped at the single-token burst.
    ///
    /// Any positive elapsed time at a budget of one or more tokens per second refills the
    /// one-token bucket completely, which is why the gain needs no multiplication here; the cap
    /// carries the burst policy.
    fn refill(state: &mut LimiterState, now: i64, global_per_second: u64) {
        let Some(last) = state.last_refill_secs else {
            state.last_refill_secs = Some(now);
            return;
        };
        let elapsed = now.saturating_sub(last);
        if elapsed == 0 {
            return;
        }
        if global_per_second > 0 {
            state.tokens_milli = ONE_TOKEN_MILLI;
        }
        state.last_refill_secs = Some(now);
    }

    /// The chat-gate answer, or `None` when the chat may compete for global budget.
    fn chat_denial(
        state: &LimiterState,
        chat_id: i64,
        now: i64,
        interval_ms: u64,
    ) -> Option<RateDecision> {
        if interval_ms == 0 {
            return None;
        }
        let last = state.last_proceed_by_chat.get(&chat_id).copied()?;
        // A clock stepped backward yields zero elapsed: the chat waits out its full interval
        // rather than racing sends across a time correction.
        let elapsed_ms = u64::try_from(now.saturating_sub(last))
            .unwrap_or(0)
            .saturating_mul(1000);
        // `then_some` would evaluate the subtraction eagerly and underflow on an elapsed chat;
        // the wait remainder only exists while the gap is still running.
        if elapsed_ms < interval_ms {
            Some(RateDecision::ChatWait {
                after_ms: interval_ms - elapsed_ms,
            })
        } else {
            None
        }
    }
}

/// Milliseconds until one token accrues at `global_per_second`, rounded up.
///
/// Valid configurations carry a budget of at least one (validation refuses zero downstream), so
/// the floor only keeps a misconfigured zero from dividing by zero here.
fn millis_for_one_token(global_per_second: u32) -> u64 {
    1000u64.div_ceil(u64::from(global_per_second.max(1)))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::{DeliveryLimiter, RateDecision};
    use crate::outbound::clock::Clock;

    /// Deterministic stand-in for [`Clock`]: the test sets the second explicitly and advances it
    /// between calls; queued jitters are handed out in order, defaulting to zero.
    ///
    /// Interior mutability (atomics/mutex instead of plain fields) is required because [`Clock`]
    /// methods take `&self`, and the test must move time BETWEEN `try_acquire` borrows.
    struct FakeClock {
        current_secs: AtomicI64,
        next_jitters: Mutex<VecDeque<u64>>,
    }

    impl FakeClock {
        fn at(secs: i64) -> Self {
            Self {
                current_secs: AtomicI64::new(secs),
                next_jitters: Mutex::new(VecDeque::new()),
            }
        }

        fn advance_secs(&self, secs: i64) {
            self.current_secs.fetch_add(secs, Ordering::Relaxed);
        }
    }

    impl Clock for FakeClock {
        fn now_secs(&self) -> i64 {
            self.current_secs.load(Ordering::Relaxed)
        }

        fn jitter_millis(&self, _bound_ms: u64) -> u64 {
            self.next_jitters
                .lock()
                .map_or(0, |mut queue| queue.pop_front().unwrap_or(0))
        }
    }

    #[test]
    fn global_bucket_refuses_calls_beyond_budget_per_window() {
        let limiter = DeliveryLimiter::new(1, 0);
        let clock = FakeClock::at(1_000_000);

        assert_eq!(limiter.try_acquire(&clock, 11), RateDecision::Proceed);
        assert_eq!(
            limiter.try_acquire(&clock, 22),
            RateDecision::GlobalWait { after_ms: 1000 },
            "the second chat must wait for the global budget to refill"
        );

        clock.advance_secs(1);
        assert_eq!(
            limiter.try_acquire(&clock, 33),
            RateDecision::Proceed,
            "one elapsed window refills exactly the single-token burst"
        );
    }

    #[test]
    fn per_chat_interval_enforces_minimum_gap() {
        // The clock ticks in whole seconds, so "immediately after" within this test is the same
        // tick as the proceed, and a 1200 ms gap is first satisfied two ticks later. The
        // discriminating behavior is unchanged: the same chat is denied while its gap runs, a
        // different chat escapes the chat gate entirely, and the same chat passes once its gap
        // has fully elapsed.
        let limiter = DeliveryLimiter::new(30, 1200);
        let clock = FakeClock::at(2_000_000);

        assert_eq!(limiter.try_acquire(&clock, 7), RateDecision::Proceed);
        assert!(
            matches!(
                limiter.try_acquire(&clock, 7),
                RateDecision::ChatWait { after_ms } if after_ms >= 700
            ),
            "the same chat inside its interval must wait at least the remainder of the gap"
        );
        assert!(
            matches!(
                limiter.try_acquire(&clock, 8),
                RateDecision::GlobalWait { .. }
            ),
            "a different chat escapes the chat gate but still answers to the global bucket"
        );

        clock.advance_secs(1);
        assert_eq!(
            limiter.try_acquire(&clock, 8),
            RateDecision::Proceed,
            "a fresh tick refills the bucket for the other chat"
        );

        clock.advance_secs(1);
        assert_eq!(
            limiter.try_acquire(&clock, 7),
            RateDecision::Proceed,
            "two ticks exceed the 1200 ms gap, so the first chat proceeds again"
        );
    }

    #[test]
    fn burst_capacity_is_one_token() {
        let limiter = DeliveryLimiter::new(30, 0);
        let clock = FakeClock::at(3_000_000);

        let decisions: Vec<RateDecision> = (0..5i64)
            .map(|offset| limiter.try_acquire(&clock, 100 + offset))
            .collect();

        assert_eq!(decisions[0], RateDecision::Proceed);
        for decision in &decisions[1..] {
            assert_eq!(
                *decision,
                RateDecision::GlobalWait { after_ms: 34 },
                "a 30/s budget refills one token in ceil(1000/30) ms; no burst beyond one"
            );
        }
    }
}
