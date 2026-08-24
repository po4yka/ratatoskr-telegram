//! The startup factory: from validated configuration to a serving public router.
//!
//! The lifecycle calls [`build`] once, after the database is prepared and before any listener
//! binds. It learns the bot identity with `get_me` — a webhook that cannot name its bot cannot key
//! its deduplication table, so the failure refuses startup — spawns the processing worker, and
//! hands back the admission router.

use std::time::Duration;

use axum::Router;
use telegram_core::{Subsystem, TelegramError};
use telegram_http::PublicContext;
use telegram_persistence::IdentityProfile;

use crate::intake::{self, Intake, IntakeSettings};

/// Build the public router for this process, or refuse startup.
///
/// # Errors
///
/// A [`TelegramError::Internal`] labelled `bot_api` when the client stack cannot be built or
/// Telegram rejects the credential (`get_me` failed); `http` when the webhook or database
/// configuration the role requires is somehow absent — unreachable behind validation V13.
pub async fn build(context: PublicContext) -> Result<Router, TelegramError> {
    let webhook = context.config.webhook.as_ref().ok_or_else(|| {
        TelegramError::internal(
            Subsystem::Http,
            std::io::Error::new(std::io::ErrorKind::NotFound, "webhook configuration absent"),
        )
    })?;
    let database = context.database.ok_or_else(|| {
        TelegramError::internal(
            Subsystem::Http,
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the intake database was not prepared",
            ),
        )
    })?;

    let bot_api = &context.config.bot_api;
    let client = bot_api::Client::new(
        &bot_api.token,
        &bot_api.base_url,
        Duration::from_secs(bot_api.timeout_seconds),
    )
    .map_err(|error| TelegramError::internal(Subsystem::BotApi, error))?;

    let me = client.get_me().await.map_err(|error| {
        // Logged once here at the boundary: which class failed, never the token or URL.
        tracing::error!(error = %error, class = "bot_identity_unavailable", "getMe failed during startup");
        TelegramError::internal(Subsystem::BotApi, error)
    })?;

    // The bot's own identity is safe telemetry: a number and its username, not user data.
    tracing::info!(
        bot_id = me.user.id.0,
        username = me.user.username.as_deref().unwrap_or(""),
        "webhook serving bot identity",
    );

    // Telegram bot ids fit u32 today; the dedupe column is bigint, so widen once here.
    let bot_id = i64::from(u32::try_from(me.user.id.0).unwrap_or(u32::MAX));

    // Design D3: the owner row exists before the first delivery is admitted, inserted only when
    // absent — an operator-disabled row survives every restart. Rule V14 guarantees the value
    // for this role; the defensive arm refuses rather than serve a policy with no principal.
    let Some(owner_id) = context.config.access.owner_telegram_user_id else {
        return Err(TelegramError::internal(
            Subsystem::Http,
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the owner telegram user id was not configured",
            ),
        ));
    };
    database
        .ensure_identity(owner_id, &IdentityProfile::default())
        .await?;
    tracing::debug!("the configured owner identity is present");

    let settings = IntakeSettings {
        secret: webhook.secret_token.clone(),
        max_body_bytes: usize::try_from(webhook.max_body_bytes).unwrap_or(usize::MAX),
        bot_id,
        queue_capacity: intake::QUEUE_CAPACITY,
    };
    let (intake, receiver) = Intake::new(settings, database);
    tokio::spawn(intake::run_worker(intake.database.clone(), receiver));

    Ok(intake.router())
}
