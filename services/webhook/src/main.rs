//! The `ratatoskr-telegram-webhook` deployable.
//!
//! Plan item 2: the process lifecycle plus the public update-intake listener. `main` is the role
//! constant, the `check-config` and `check-schema` pre-flights, the `assertion-key` operator
//! command, and one call into the shared harness — the intake pipeline itself lives in the library
//! so tests drive it without spawning a process.

use std::process::ExitCode;

use telegram_core::RuntimeRole;
use telegram_http::PublicRoutes;

const ROLE: RuntimeRole = RuntimeRole::Webhook;

#[tokio::main]
async fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("check-config") => return telegram_http::check_config(ROLE),
        Some("check-schema") => return telegram_http::check_schema(ROLE).await,
        Some("assertion-key") => {
            let rest: Vec<String> = arguments.collect();
            return ratatoskr_telegram_webhook::assertion_key_command::run(&rest);
        }
        // An argument the binary does not know must not silently start the service.
        Some(_) => return ratatoskr_telegram_webhook::assertion_key_command::usage(),
        None => {}
    }
    telegram_http::run(
        ROLE,
        PublicRoutes::new(ratatoskr_telegram_webhook::intake::build),
    )
    .await
}
