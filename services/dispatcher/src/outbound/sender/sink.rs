//! The wire seam: the two Bot API writes the sender performs, behind an object-safe trait.
//!
//! Native `async fn` in traits is not dyn-compatible, and no async-trait dependency may be
//! added, so each method returns a hand-boxed future owning a cloned client handle. The seam
//! exists so delivery tests drive a recording fake while production wraps [`bot_api::Client`] —
//! the only crate allowed to touch teloxide.

use std::future::Future;
use std::pin::Pin;

use bot_api::{BotApiError, ChatId, MessageId};

/// The one field of a Bot API acknowledgment the dispatcher persists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentMessage {
    /// The Telegram message id the provider returned.
    pub message_id: i64,
}

/// A boxed send future, so [`BotApiSink`] stays object-safe without an async-trait dependency.
pub type SendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SentMessage, BotApiError>> + Send + 'a>>;

/// The two Bot API writes the dispatcher performs today. Grown by a new method when a flow needs
/// a third write — never by leaking a concrete client type into the sender.
pub trait BotApiSink: Send + Sync {
    /// Deliver a fresh message to the chat.
    fn send_message(&self, chat_id: i64, text: &str) -> SendFuture<'_>;

    /// Replace one sent message's text in place.
    fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> SendFuture<'_>;
}

/// The production sink over the typed Bot API client.
#[derive(Debug, Clone)]
pub struct ClientSink {
    client: bot_api::Client,
}

impl ClientSink {
    /// Wrap an authenticated client.
    #[must_use]
    pub fn new(client: bot_api::Client) -> Self {
        Self { client }
    }
}

impl BotApiSink for ClientSink {
    fn send_message(&self, chat_id: i64, text: &str) -> SendFuture<'_> {
        // The future owns the client handle and the text so it fits the returned lifetime
        // without borrowing `self`; cloning the client is cheap (an Arc underneath).
        let client = self.client.clone();
        let text = text.to_owned();
        Box::pin(async move {
            client
                .send_message(ChatId(chat_id), &text)
                .await
                .map(|message| map_sent(&message))
        })
    }

    fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> SendFuture<'_> {
        let client = self.client.clone();
        let text = text.to_owned();
        Box::pin(async move {
            // Telegram message ids fit `i32` today; a stored id beyond that cannot name a real
            // message, and 0 is rejected by the API as cleanly invalid rather than silently
            // editing some other message.
            let wire_id = MessageId(i32::try_from(message_id).unwrap_or(0));
            client
                .edit_message_text(ChatId(chat_id), wire_id, &text)
                .await
                .map(|message| map_sent(&message))
        })
    }
}

/// Project the teloxide message onto the only field persistence stores.
fn map_sent(message: &bot_api::Message) -> SentMessage {
    SentMessage {
        message_id: i64::from(message.id.0),
    }
}
