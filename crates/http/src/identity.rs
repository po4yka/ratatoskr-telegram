//! Binding a Telegram sender to the Platform user a session exchange authenticated.
//!
//! Platform's session exchange is the only place the Platform user id for a Telegram sender is
//! learned, so both roles run the same hook after each network exchange (XR-021 CONTRACTS S11).

use std::sync::Arc;

use platform_api::session::OnMint;
use telegram_persistence::Database;
use telegram_telemetry::metrics::TELEGRAM_IDENTITY_BINDING_TOTAL;

/// The hook a `SessionSource` runs after every successful network exchange.
///
/// The subject is the sender's Telegram user id. The hook writes the Platform user id to
/// `telegram.identities.internal_user_id` and counts the outcome (`bound`, `unchanged`, or
/// `failed`). It never fails the exchange: a failed write is logged by class and the binding is
/// retried at the sender's next network exchange.
#[must_use]
pub fn identity_binding_hook(database: Database) -> OnMint {
    Arc::new(move |subject, user_id| {
        let database = database.clone();
        Box::pin(async move {
            let outcome = if let Ok(telegram_user_id) = subject.parse::<i64>() {
                match database.bind_internal_user(telegram_user_id, user_id).await {
                    Ok(true) => "bound",
                    Ok(false) => "unchanged",
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            class = "identity_binding_failed",
                            "the Platform user could not be bound to the Telegram identity",
                        );
                        "failed"
                    }
                }
            } else {
                tracing::warn!(
                    class = "identity_binding_failed",
                    "the session subject is not a Telegram user id",
                );
                "failed"
            };
            metrics::counter!(TELEGRAM_IDENTITY_BINDING_TOTAL, "outcome" => outcome).increment(1);
        })
    })
}
