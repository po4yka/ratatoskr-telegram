//! The typed Telegram Bot API client boundary: every call this service makes to Telegram goes
//! through here and nowhere else.
//!
//! [`Client`] exposes the six methods the interaction flows need — `get_me`, `set_webhook`,
//! `send_message`, `edit_message_text`, `answer_callback_query`, `send_chat_action` — over the
//! pinned `teloxide` dependency. Failures surface as [`BotApiError`], a closed taxonomy of
//! network, rate-limit, API and parse classes; nothing in its rendering carries the bot token,
//! which travels only inside request URL paths and is never logged by this crate.
//!
//! Update payloads deserialize into the re-exported teloxide types ([`Update`], [`UpdateKind`]):
//! a well-formed envelope with an unknown kind parses as [`UpdateKind::Error`] — unsupported input,
//! not malformed input — which is exactly the split admission needs.
//!
//! Tests never contact Telegram: the base URL points at a harness server serving recorded
//! fixtures.

use std::time::Duration;

use secrecy::{ExposeSecret as _, SecretString};
use teloxide::requests::{Request as _, Requester};
use thiserror::Error as ThisError;
use url::Url;

pub use teloxide::types::{
    CallbackQuery, Chat, ChatAction, ChatId, ChatKind, MaybeInaccessibleMessage, Me, Message,
    MessageId, Update, UpdateKind, User,
};

/// Presentation beyond plain text for one outgoing message.
///
/// Deliberately wire-neutral: parse mode is the API's label (`HTML`), the keyboard is the exact
/// JSON the API expects under `reply_markup`. Keeping this crate free of higher-level rendering
/// types lets the outbound queue store the payload verbatim and every layer pass it through.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageOptions {
    /// The parse mode label, e.g. `HTML`.
    pub parse_mode: Option<String>,
    /// The inline keyboard layout as the Bot API's `reply_markup` JSON.
    pub reply_markup: Option<serde_json::Value>,
}

impl MessageOptions {
    /// HTML parse mode without a keyboard.
    #[must_use]
    pub fn html() -> Self {
        Self {
            parse_mode: Some("HTML".to_owned()),
            reply_markup: None,
        }
    }
}

/// Why one Bot API call failed. Closed, extendable, and safe to render: no variant carries the
/// token or a full response body.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum BotApiError {
    /// Transport failed: connection refused, DNS, reset stream, or the configured timeout fired.
    /// Retryable after backoff.
    #[error("the Bot API endpoint could not be reached")]
    Network(#[source] Arc<reqwest::Error>),

    /// Telegram asked for a pause (`429` + `retry_after`). The delay is authoritative.
    #[error("the Bot API rate limit was hit")]
    RateLimited {
        /// How long to wait before repeating the call.
        retry_after: Duration,
    },

    /// Telegram answered with an error body. The description is theirs, one line, no secrets.
    #[error("the Bot API rejected the call: {description}")]
    Api {
        /// Telegram's own error text (e.g. `Bad Request: chat not found`).
        description: String,
    },

    /// The group migrated to a supergroup; repeat against the new chat id.
    #[error("the chat migrated to a supergroup")]
    ChatMigrated {
        /// The supergroup chat id Telegram redirected to.
        to: ChatId,
    },

    /// A response could not be parsed as the Bot API answered it. Never retryable as-is.
    #[error("a Bot API response could not be parsed")]
    Json,

    /// A local file transfer failed underneath a call. No method this crate exposes today moves
    /// files, so this rides along for totality rather than pretending a transport or API failure
    /// happened.
    #[error("a local file transfer failed")]
    Io(#[source] Arc<std::io::Error>),
}

use std::sync::Arc;

impl BotApiError {
    /// Maps the client library's error onto the taxonomy.
    fn map(error: teloxide::RequestError) -> Self {
        match error {
            teloxide::RequestError::RetryAfter(seconds) => Self::RateLimited {
                retry_after: seconds.duration(),
            },
            teloxide::RequestError::MigrateToChatId(chat_id) => Self::ChatMigrated { to: chat_id },
            teloxide::RequestError::Api(api_error) => Self::Api {
                description: api_error.to_string(),
            },
            teloxide::RequestError::Network(source) => Self::Network(source),
            teloxide::RequestError::InvalidJson { source: _, raw: _ } => Self::Json,
            teloxide::RequestError::Io(source) => Self::Io(source),
        }
    }
}

/// One authenticated handle over the Bot API. Cheap to clone; share it.
#[derive(Debug, Clone)]
pub struct Client {
    bot: teloxide::Bot,
}

impl Client {
    /// Build a client against `base_url` with `timeout` as the whole-call budget.
    ///
    /// `base_url` is the deployment's Bot API origin — `https://api.telegram.org` in production, a
    /// loopback harness in tests (rule V9 keeps that choice honest). The token lives in request
    /// paths by Bot API contract; this crate never logs URLs.
    ///
    /// # Errors
    ///
    /// [`BotApiError::Network`] when the HTTP client stack cannot be built (TLS backend
    /// initialisation), or [`BotApiError::Json`] when `base_url` does not parse — unreachable for a
    /// configuration that passed validation, kept total anyway.
    pub fn new(
        token: &SecretString,
        base_url: &Url,
        timeout: Duration,
    ) -> Result<Self, BotApiError> {
        let http = reqwest::ClientBuilder::new()
            .timeout(timeout)
            .use_rustls_tls()
            .build()
            .map_err(|error| BotApiError::Network(Arc::new(error)))?;

        // teloxide appends `/bot<token>/<method>` to this origin; a trailing slash would double up
        // the separator exactly as it refuses to in its own env loader.
        let trimmed = base_url.as_str().trim_end_matches('/');
        let api_url = reqwest::Url::parse(trimmed).map_err(|_| BotApiError::Json)?;

        Ok(Self {
            bot: teloxide::Bot::with_client(token.expose_secret(), http).set_api_url(api_url),
        })
    }

    /// The bot's own identity. Startup calls this once: it validates the credential and yields the
    /// bot id update deduplication keys on.
    ///
    /// # Errors
    ///
    /// As the taxonomy above.
    pub async fn get_me(&self) -> Result<Me, BotApiError> {
        self.bot.get_me().send().await.map_err(BotApiError::map)
    }

    /// Register the delivery URL Telegram should POST updates to, optionally carrying the webhook
    /// secret Telegram will echo back on every delivery.
    ///
    /// An operational write by nature: callers invoke it explicitly, never as a side effect.
    ///
    /// # Errors
    ///
    /// As the taxonomy above.
    pub async fn set_webhook(
        &self,
        url: &Url,
        secret: Option<&SecretString>,
    ) -> Result<(), BotApiError> {
        use teloxide::payloads::SetWebhookSetters as _;

        let request = self.bot.set_webhook(url.clone());
        let request = match secret {
            Some(secret) => request.secret_token(secret.expose_secret()),
            None => request,
        };
        request.send().await.map(|_| ()).map_err(BotApiError::map)
    }

    /// Send a message, optionally carrying parse mode and an inline keyboard.
    ///
    /// # Errors
    ///
    /// As the taxonomy above.
    pub async fn send_message(
        &self,
        chat_id: ChatId,
        text: &str,
        options: Option<&MessageOptions>,
    ) -> Result<Message, BotApiError> {
        use teloxide::payloads::SendMessageSetters as _;

        let request = self.bot.send_message(chat_id, text);
        let request = match options {
            Some(o) => {
                let request = match o.parse_mode.as_deref() {
                    Some("HTML") => request.parse_mode(teloxide::types::ParseMode::Html),
                    _ => request,
                };
                match &o.reply_markup {
                    Some(markup) => request.reply_markup(keyboard_of(markup)?),
                    None => request,
                }
            }
            None => request,
        };
        request.send().await.map_err(BotApiError::map)
    }

    /// Replace one sent message's text - and its presentation - in place.
    ///
    /// # Errors
    ///
    /// As the taxonomy above; `message is not modified` arrives as an [`BotApiError::Api`].
    pub async fn edit_message_text(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        text: &str,
        options: Option<&MessageOptions>,
    ) -> Result<Message, BotApiError> {
        use teloxide::payloads::EditMessageTextSetters as _;

        let request = self.bot.edit_message_text(chat_id, message_id, text);
        let request = match options {
            Some(o) => {
                let request = match o.parse_mode.as_deref() {
                    Some("HTML") => request.parse_mode(teloxide::types::ParseMode::Html),
                    _ => request,
                };
                match &o.reply_markup {
                    Some(markup) => request.reply_markup(keyboard_of(markup)?),
                    None => request,
                }
            }
            None => request,
        };
        request.send().await.map_err(BotApiError::map)
    }

    /// Acknowledge a callback query so the sender's spinner stops.
    ///
    /// # Errors
    ///
    /// As the taxonomy above.
    pub async fn answer_callback_query(&self, callback_query_id: &str) -> Result<(), BotApiError> {
        use teloxide::types::CallbackQueryId;
        self.bot
            .answer_callback_query(CallbackQueryId(callback_query_id.to_owned()))
            .send()
            .await
            .map(|_| ())
            .map_err(BotApiError::map)
    }

    /// Show a transient activity indicator in a chat (e.g. `typing`).
    ///
    /// # Errors
    ///
    /// As the taxonomy above.
    pub async fn send_chat_action(
        &self,
        chat_id: ChatId,
        action: ChatAction,
    ) -> Result<(), BotApiError> {
        self.bot
            .send_chat_action(chat_id, action)
            .send()
            .await
            .map(|_| ())
            .map_err(BotApiError::map)
    }
}

/// Deserialize a stored keyboard layout into teloxide's typed markup.
///
/// # Errors
///
/// [`BotApiError::Json`] when the value is not a keyboard layout, which would mean a stored
/// payload was corrupted rather than bad input here.
fn keyboard_of(
    value: &serde_json::Value,
) -> Result<teloxide::types::InlineKeyboardMarkup, BotApiError> {
    serde_json::from_value(value.clone()).map_err(|_| BotApiError::Json)
}
