//! The `ratatoskr-telegram-dispatcher` deployable.
//!
//! Plan item 1: the process lifecycle only — typed configuration, telemetry, the operator plane,
//! and the owned `telegram` schema applied to a configured database. Event consumption, per-chat
//! ordering and Bot API delivery arrive with later plan items; nothing here contacts Telegram.

use std::process::ExitCode;

use telegram_core::RuntimeRole;

const ROLE: RuntimeRole = RuntimeRole::Dispatcher;

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("check-config") {
        return telegram_http::check_config(ROLE);
    }
    telegram_http::run(ROLE).await
}
