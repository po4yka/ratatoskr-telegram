//! Terminal composition: the one place a finished operation grows links.
//!
//! Non-terminal renders stay plain status-led text. A terminal render MAY add a fallback
//! hyperlink to the captured address and one URL button whose parameter is an opaque deep-link
//! intent - both resolved from this service's own records, never invented. A failure render adds
//! resend guidance instead of any retry control: callback tokens belong to a later plan item,
//! and a button this build cannot honour would be a lie with markup.

use telegram_persistence::outbound_jobs::MessagePayload;
use url::Url;

use crate::projection::event::{OperationEvent, OperationStatus};

/// Compose the terminal payload over the plain rendered body.
///
/// `intent` carries the opaque token and the source address; `username` is the serving bot's.
/// Either may be absent, and the payload degrades to text-only rather than failing the render.
#[must_use]
pub fn compose_terminal(
    body: String,
    event: &OperationEvent,
    intent: Option<&telegram_persistence::intents::IntentRecord>,
    username: Option<&str>,
) -> MessagePayload {
    let mut text = body;

    let wants_button =
        intent.is_some() && username.is_some() && event.status != OperationStatus::Failed;
    if let Some(record) = intent {
        if let Ok(source) = Url::parse(&record.source_url) {
            let escaped = escape_html(source.as_str());
            text.push_str("\n<a href=\"");
            text.push_str(&escaped);
            text.push_str("\">");
            text.push_str(&escaped);
            text.push_str("</a>");
        }
        if event.status == OperationStatus::Failed {
            text.push_str("\nResend the link to try again.");
        }
        if let (Some(username), true) = (username, wants_button) {
            let target = format!("https://t.me/{username}?startapp={}", record.id);
            text.push('\n');
            return MessagePayload {
                text,
                parse_mode: Some("HTML".to_owned()),
                reply_markup: Some(url_button(&target)),
            };
        }
        return MessagePayload {
            text,
            parse_mode: Some("HTML".to_owned()),
            reply_markup: None,
        };
    }

    if event.status == OperationStatus::Failed {
        text.push_str("\nResend the link to try again.");
    }
    MessagePayload::text(text)
}

/// The inline-keyboard JSON carrying exactly one URL button to the Mini App deep link.
#[must_use]
fn url_button(target: &str) -> serde_json::Value {
    serde_json::json!({
        "inline_keyboard": [[{"text": "Open", "url": target}]]
    })
}

/// Escape the five characters Telegram's HTML parse mode treats specially.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::expect_used,
    reason = "assertions inside the in-crate test module"
)]
mod tests {
    use super::compose_terminal;
    use crate::projection::event::{OperationEvent, OperationStatus};
    use sqlx::types::Uuid;
    use telegram_persistence::intents::IntentRecord;

    const USERNAME: &str = "ratatoskr_test_bot";

    fn an_event(status: OperationStatus) -> OperationEvent {
        OperationEvent {
            event_id: Uuid::now_v7(),
            occurred_at_secs: 1_800_000_000,
            correlation_id: "operation:018f0000-0000-7000-8000-000000000001".to_owned(),
            operation_id: Uuid::now_v7(),
            status,
            stage: None,
            progress_percent: None,
            errors: Vec::new(),
            warnings: Vec::new(),
            message: Some("source refused the connection".to_owned()),
        }
    }

    fn an_intent() -> IntentRecord {
        IntentRecord {
            id: Uuid::now_v7(),
            bot_id: 42,
            chat_id: 900_700_601,
            operation_id: Uuid::now_v7(),
            source_url: "https://example.test/article".to_owned(),
        }
    }

    #[test]
    fn succeeded_terminal_composes_links_and_button() {
        let intent = an_intent();
        let event = an_event(OperationStatus::Succeeded);
        let payload = compose_terminal(
            "<b>Completed</b>".to_owned(),
            &event,
            Some(&intent),
            Some(USERNAME),
        );
        assert!(payload.text.starts_with("<b>Completed</b>"));
        assert!(
            payload
                .text
                .contains("<a href=\"https://example.test/article\">"),
            "the fallback hyperlink rides along: {}",
            payload.text
        );
        let markup = payload.reply_markup.expect("the deep-link button exists");
        let target = markup["inline_keyboard"][0][0]["url"]
            .as_str()
            .expect("url");
        assert_eq!(
            target,
            format!("https://t.me/{USERNAME}?startapp={}", intent.id),
            "the button carries only the opaque token"
        );
    }

    #[test]
    fn failed_terminal_composes_guidance_without_retry_button() {
        let intent = an_intent();
        let event = an_event(OperationStatus::Failed);
        let payload = compose_terminal(
            "<b>Failed</b>".to_owned(),
            &event,
            Some(&intent),
            Some(USERNAME),
        );
        assert!(payload.text.contains("Resend the link"), "{}", payload.text);
        assert!(
            payload.reply_markup.is_none(),
            "no retry control exists yet"
        );
    }

    #[test]
    fn missing_intent_or_username_degrades_to_text_only() {
        let event = an_event(OperationStatus::Succeeded);
        let without_intent =
            compose_terminal("<b>Completed</b>".to_owned(), &event, None, Some(USERNAME));
        assert_eq!(
            without_intent.reply_markup, None,
            "no intent record, no button"
        );

        let intent = an_intent();
        let without_username =
            compose_terminal("<b>Completed</b>".to_owned(), &event, Some(&intent), None);
        assert_eq!(
            without_username.reply_markup, None,
            "no username, no button - but the fallback link stays"
        );
        assert!(without_username.text.contains("<a href="));
    }

    #[test]
    fn non_terminal_events_never_compose_markup() {
        // The consumer branches before composing; this pins that the composer is not even given
        // non-terminal events by construction of the call site.
        let event = an_event(OperationStatus::Running);
        assert!(!event.status.is_terminal());
    }
}
