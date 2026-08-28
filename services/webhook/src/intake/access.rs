//! The authorization gate: whether a claimed update may reach domain processing at all.
//!
//! Policy inputs are persisted facts, never the delivery: an identity record that says nothing
//! admits nobody. The gate reads identity state, creates the private chat row when it admits,
//! and never writes an identity row — enrollment is bootstrap-plus-operator (design D3), never
//! first-contact. A refusal settles silently in [`crate::intake::worker`]; this module only
//! decides.

use bot_api::{ChatKind, MaybeInaccessibleMessage, Update, UpdateKind};
use telegram_persistence::{AccessState, Database};

/// Why one update was refused. Closed vocabulary — these strings are metric labels and log
/// classes, so no delivery can invent one.
#[derive(Debug, Clone, Copy)]
pub(super) enum DenialClass {
    /// No identity record carries the sender.
    UnknownSender,
    /// The sender's record is disabled; externally identical to unknown (design D6).
    DisabledIdentity,
    /// A known private chat has been disabled by the access policy.
    DisabledChat,
    /// The conversation is not a private chat, or no chat context is resolvable.
    NonPrivateChat,
}

impl DenialClass {
    /// The label value.
    #[must_use]
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownSender => "unknown_sender",
            Self::DisabledIdentity => "disabled_identity",
            Self::DisabledChat => "disabled_chat",
            Self::NonPrivateChat => "non_private_chat",
        }
    }
}

/// Evaluate the policy for one processable update.
///
/// `Ok(None)` admits: the sender is enabled, the conversation is private, and its chat row now
/// exists. `Ok(Some(class))` refuses with the reason class; the caller owes that update a silent
/// `denied` settlement and nothing else. Unsupported kinds never reach this function — they
/// settle as `unsupported` without consulting any principal.
///
/// # Errors
///
/// [`telegram_persistence::PersistenceError`] when a binding read or write fails; the worker
/// records a processing failure rather than improvising a verdict from an unreadable policy.
pub(crate) async fn authorize(
    database: &Database,
    update: &Update,
) -> Result<Option<DenialClass>, telegram_persistence::PersistenceError> {
    let (sender, chat) = match &update.kind {
        UpdateKind::Message(message) | UpdateKind::EditedMessage(message) => {
            (message.from.as_ref(), Some(&message.chat))
        }
        UpdateKind::CallbackQuery(callback) => (
            Some(&callback.from),
            callback.message.as_ref().map(resolvable_chat),
        ),
        _ => return Ok(None),
    };

    // Telegram user ids are positive and fit i64 by construction; anything else cannot name our
    // sender and is refused before it can reach a query at all.
    let Some(sender) = sender else {
        return Ok(Some(DenialClass::UnknownSender));
    };
    let Ok(sender_id) = i64::try_from(sender.id.0) else {
        return Ok(Some(DenialClass::UnknownSender));
    };

    match database
        .find_identity(sender_id)
        .await?
        .map(|identity| identity.access_state)
    {
        Some(AccessState::Enabled) => {}
        Some(AccessState::Disabled) => return Ok(Some(DenialClass::DisabledIdentity)),
        None => return Ok(Some(DenialClass::UnknownSender)),
    }

    let Some(chat) = chat else {
        return Ok(Some(DenialClass::NonPrivateChat));
    };
    if !matches!(chat.kind, ChatKind::Private { .. }) {
        // Refused without a record: groups gain no chat row (design D5).
        return Ok(Some(DenialClass::NonPrivateChat));
    }

    let chat_id = chat.id.0;
    let admitted_chat = database.ensure_chat(chat_id).await?;
    if admitted_chat.access_state != AccessState::Enabled {
        return Ok(Some(DenialClass::DisabledChat));
    }
    database.bind_private_chat(sender_id, chat_id).await?;
    Ok(None)
}

/// The chat a callback answer belongs to, when the callback still references its message.
fn resolvable_chat(message: &MaybeInaccessibleMessage) -> &bot_api::Chat {
    match message {
        MaybeInaccessibleMessage::Regular(message) => &message.chat,
        MaybeInaccessibleMessage::Inaccessible(inner) => &inner.chat,
    }
}
