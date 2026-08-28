//! The `ratatoskr-telegram-dispatcher` deployable.
//!
//! The process lifecycle only: typed configuration, telemetry, the operator plane, the owned
//! `telegram` schema applied to a configured database, and — since item 4 — the background
//! workers (durable-queue sender and projection consumer) that the lifecycle starts through the
//! background hook once its checks have passed. Nothing here contacts Telegram directly; every
//! Bot API write goes through the durable queue.

use std::process::ExitCode;

use ratatoskr_telegram_dispatcher::build;
use telegram_core::RuntimeRole;
use telegram_http::{Background, PublicRoutes};

const ROLE: RuntimeRole = RuntimeRole::Dispatcher;

#[tokio::main]
async fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("check-config") => return telegram_http::check_config(ROLE),
        Some("check-schema") => return telegram_http::check_schema(ROLE).await,
        _ => {}
    }
    // No public listener: the dispatcher sends, it does not receive. Its workers start through
    // the background factory after validation and database preparation succeed.
    telegram_http::run_with_background(ROLE, PublicRoutes::none(), Background::new(build::build))
        .await
}
