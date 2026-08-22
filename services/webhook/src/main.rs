//! The `ratatoskr-telegram-webhook` deployable.
//!
//! Plan item 2: the process lifecycle plus the public update-intake listener. `main` is the role
//! constant, a `check-config` pre-flight, and one call into the shared harness — the intake
//! pipeline itself lives in the library so tests drive it without spawning a process.

use std::process::ExitCode;

use telegram_core::RuntimeRole;
use telegram_http::PublicRoutes;

const ROLE: RuntimeRole = RuntimeRole::Webhook;

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("check-config") {
        return telegram_http::check_config(ROLE);
    }
    telegram_http::run(
        ROLE,
        PublicRoutes::new(ratatoskr_telegram_webhook::intake::build),
    )
    .await
}
