//! The webhook deployable: Bot API update intake, admission, deduplication, fast acknowledgment.
//!
//! The binary is a role constant and the shared lifecycle; everything specific lives in the
//! library so the admission suite can drive it without spawning a process. [`intake::build`] is
//! the startup factory the lifecycle calls: it learns the bot identity with `get_me`, spawns the
//! processing worker, and returns the public router.
//!
//! Nothing here parses commands or touches domain state — that is plan items 3+.

pub mod intake;

/// The webhook runtime role this package compiles.
pub const ROLE: telegram_core::RuntimeRole = telegram_core::RuntimeRole::Webhook;
