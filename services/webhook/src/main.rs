//! The `ratatoskr-telegram-webhook` deployable.
//!
//! Plan item 1: the process lifecycle only — typed configuration, telemetry, the operator plane,
//! and the owned `telegram` schema applied to a configured database. The Bot API listener, update
//! deduplication and command handling arrive with the next plan items; nothing here contacts
//! Telegram.

use std::process::ExitCode;

use telegram_core::RuntimeRole;

const ROLE: RuntimeRole = RuntimeRole::Webhook;

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("check-config") {
        return telegram_http::check_config(ROLE);
    }
    telegram_http::run(ROLE).await
}
