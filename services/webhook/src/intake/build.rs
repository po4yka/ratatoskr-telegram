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
    let capture = build_capture_context(&context.config.platform)?;
    let (intake, receiver) = Intake::new(settings, database);
    tokio::spawn(intake::run_worker(
        intake.database.clone(),
        receiver,
        Some(capture),
    ));

    Ok(intake.router())
}

/// Build the Platform half of the domain action from validated configuration.
///
/// # Errors
///
/// A [`TelegramError::Internal`] labelled `platform` when the client stack cannot be built or
/// the configured signing key does not decode — both unreachable behind validation V16/V17.
fn build_capture_context(
    platform: &telegram_core::PlatformConfig,
) -> Result<intake::worker::CaptureContext, TelegramError> {
    use secrecy::ExposeSecret as _;

    let seed = decode_seed(platform.assertion_signing_key.expose_secret())?;
    let issuer = platform_api::assertion::AssertionIssuer::from_seed(&seed, &platform.audience)
        .map_err(|error| TelegramError::internal(Subsystem::Platform, error))?;
    let client = platform_api::Client::new(
        &platform.base_url,
        Duration::from_secs(platform.timeout_seconds),
    )
    .map_err(|error| TelegramError::internal(Subsystem::Platform, error))?;
    let sessions = platform_api::session::SessionSource::new(
        client,
        issuer,
        Box::new(platform_api::session::SystemClock),
    );
    Ok(intake::worker::CaptureContext::new(std::sync::Arc::new(
        sessions,
    )))
}

/// Decode the configured 64-hex-character Ed25519 seed into its 32 bytes.
fn decode_seed(hex_key: &str) -> Result<[u8; 32], TelegramError> {
    fn digit(character: u8) -> Option<u8> {
        match character {
            b'0'..=b'9' => Some(character - b'0'),
            b'a'..=b'f' => Some(character - b'a' + 10),
            b'A'..=b'F' => Some(character - b'A' + 10),
            _ => None,
        }
    }
    let bytes = hex_key.as_bytes();
    if bytes.len() != 64 {
        return Err(TelegramError::internal(
            Subsystem::Platform,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the assertion signing key must be 64 hex characters",
            ),
        ));
    }
    let mut seed = [0u8; 32];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        let high = digit(pair[0]).ok_or_else(|| {
            TelegramError::internal(
                Subsystem::Platform,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "bad signing-key hex"),
            )
        })?;
        let low = digit(pair[1]).ok_or_else(|| {
            TelegramError::internal(
                Subsystem::Platform,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "bad signing-key hex"),
            )
        })?;
        seed[index] = (high << 4) | low;
    }
    Ok(seed)
}
