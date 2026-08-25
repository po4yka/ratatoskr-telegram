//! The dispatcher deployable: durable outbound delivery and operation projections.
//!
//! The binary is a role constant and the shared lifecycle; everything specific lives in the
//! library so the delivery suites can drive it without spawning a process, mirroring the webhook
//! package split. The startup factory (`build.rs`) and the worker modules land with their own
//! test-first pairs of the dispatcher change.

pub mod outbound;
pub mod projection;

/// The dispatcher runtime role this package compiles.
pub const ROLE: telegram_core::RuntimeRole = telegram_core::RuntimeRole::Dispatcher;
