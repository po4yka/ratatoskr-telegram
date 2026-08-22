//! Runtime role, typed Telegram configuration, and the Telegram error taxonomy.
//!
//! This crate is the part of `ratatoskr-telegram` that every other in-repo crate may depend on: it
//! has no axum, no OpenTelemetry and no HTTP server, so persistence and telemetry can use
//! [`TelegramError`] and [`TelegramConfig`] without linking a web framework.
//!
//! - [`role`] — [`RuntimeRole`], the deployment axis. Fixed by the binary, never read from the
//!   environment.
//! - [`config`] — the typed tree, the `RATATOSKR__` loader, and the startup rules a process must
//!   satisfy before it binds anything.
//! - [`error`] — [`TelegramError`] and the bounded subsystem labels used by telemetry.

pub mod config;
pub mod error;
pub mod role;

pub use crate::config::{ConfigError, DatabaseConfig, TelegramConfig, Violation};
pub use crate::error::{Subsystem, TelegramError};
pub use crate::role::RuntimeRole;
