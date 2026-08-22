//! Update classification: the closed label vocabulary and the kinds this build acts on.

use bot_api::UpdateKind;

/// The CLOSED classification label for one update kind: the Bot API wire key for known kinds,
/// `unsupported` for anything the envelope preserved but this build does not model. These strings
/// reach metric labels and database rows, so a delivery can never mint a new one.
#[must_use]
pub fn kind_label(kind: &UpdateKind) -> &'static str {
    match kind {
        UpdateKind::Message(_) => "message",
        UpdateKind::EditedMessage(_) => "edited_message",
        UpdateKind::ChannelPost(_) => "channel_post",
        UpdateKind::EditedChannelPost(_) => "edited_channel_post",
        UpdateKind::BusinessConnection(_) => "business_connection",
        UpdateKind::BusinessMessage(_) => "business_message",
        UpdateKind::EditedBusinessMessage(_) => "edited_business_message",
        UpdateKind::DeletedBusinessMessages(_) => "deleted_business_messages",
        UpdateKind::MessageReaction(_) => "message_reaction",
        UpdateKind::MessageReactionCount(_) => "message_reaction_count",
        UpdateKind::InlineQuery(_) => "inline_query",
        UpdateKind::ChosenInlineResult(_) => "chosen_inline_result",
        UpdateKind::CallbackQuery(_) => "callback_query",
        UpdateKind::ShippingQuery(_) => "shipping_query",
        UpdateKind::PreCheckoutQuery(_) => "pre_checkout_query",
        UpdateKind::PurchasedPaidMedia(_) => "purchased_paid_media",
        UpdateKind::Poll(_) => "poll",
        UpdateKind::PollAnswer(_) => "poll_answer",
        UpdateKind::MyChatMember(_) => "my_chat_member",
        UpdateKind::ChatMember(_) => "chat_member",
        UpdateKind::ChatJoinRequest(_) => "chat_join_request",
        UpdateKind::ChatBoost(_) => "chat_boost",
        UpdateKind::RemovedChatBoost(_) => "removed_chat_boost",
        // A well-formed envelope whose kind is unknown to this build (or failed its own inner
        // parse). Supported input, unsupported kind — never "malformed".
        UpdateKind::Error(_) => "unsupported",
    }
}

/// Whether this build acts on the kind. Plan items 3+ grow this set; everything else settles as
/// `unsupported` so the row records what arrived without pretending it was handled.
#[must_use]
pub const fn supported(kind: &UpdateKind) -> bool {
    matches!(
        kind,
        UpdateKind::Message(_) | UpdateKind::EditedMessage(_) | UpdateKind::CallbackQuery(_)
    )
}
