//! Deterministic notification-preference commands for one authorized private chat.

use telegram_persistence::{
    Database, NotificationPreferenceUpdate, NotificationPreferences, PersistenceError, QuietPolicy,
};

use super::worker::now_secs;

const KNOWN_CLASSES: &[&str] = ratatoskr_notification_contracts::NotificationClass::KNOWN;

/// Whether a message was a settings command and, if so, whether it was processed durably.
pub(crate) enum SettingsResult {
    /// The text belongs to another command grammar.
    NotSettings,
    /// A settings reply was durably queued.
    Processed,
    /// Preference or queue persistence failed.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    NotSettings,
    Inspect,
    SetGlobal(bool),
    SetClass(String, Option<bool>),
    SetQuiet(QuietPolicy),
    SetBypass(bool),
    Invalid,
}

/// Apply one exact settings form and enqueue one direct response.
pub(crate) async fn handle(
    database: &Database,
    bot_id: i64,
    telegram_user_id: i64,
    chat_id: i64,
    text: &str,
) -> SettingsResult {
    let command = parse(text);
    if command == Command::NotSettings {
        return SettingsResult::NotSettings;
    }
    if command == Command::Invalid {
        return if enqueue(database, bot_id, chat_id, usage(), now_secs())
            .await
            .is_ok()
        {
            SettingsResult::Processed
        } else {
            SettingsResult::Failed
        };
    }
    if command == Command::Inspect {
        return render_current(database, bot_id, telegram_user_id, chat_id).await;
    }
    let Ok(Some(current)) = database
        .notification_preferences(telegram_user_id, chat_id)
        .await
    else {
        return SettingsResult::Failed;
    };
    let mut update = NotificationPreferenceUpdate {
        enabled: current.enabled,
        quiet_policy: current.quiet_policy,
        high_priority_bypass: current.high_priority_bypass,
        class_override: None,
    };
    match command {
        Command::SetGlobal(enabled) => update.enabled = enabled,
        Command::SetClass(class, enabled) => update.class_override = Some((class, enabled)),
        Command::SetQuiet(policy) => update.quiet_policy = policy,
        Command::SetBypass(enabled) => update.high_priority_bypass = enabled,
        Command::NotSettings | Command::Inspect | Command::Invalid => {
            return SettingsResult::Failed;
        }
    }
    if database
        .update_notification_preferences(
            telegram_user_id,
            chat_id,
            current.version,
            &update,
            now_secs(),
        )
        .await
        .is_err()
    {
        return SettingsResult::Failed;
    }
    render_current(database, bot_id, telegram_user_id, chat_id).await
}

async fn render_current(
    database: &Database,
    bot_id: i64,
    telegram_user_id: i64,
    chat_id: i64,
) -> SettingsResult {
    let Ok(Some(current)) = database
        .notification_preferences(telegram_user_id, chat_id)
        .await
    else {
        return SettingsResult::Failed;
    };
    if enqueue(database, bot_id, chat_id, render(&current), now_secs())
        .await
        .is_ok()
    {
        SettingsResult::Processed
    } else {
        SettingsResult::Failed
    }
}

fn parse(text: &str) -> Command {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.first().copied() != Some("/settings") {
        return Command::NotSettings;
    }
    match words.as_slice() {
        ["/settings"] => Command::Inspect,
        ["/settings", "notifications", value] => {
            bool_value(value).map_or(Command::Invalid, Command::SetGlobal)
        }
        ["/settings", "notification", class, value] if KNOWN_CLASSES.contains(class) => {
            let value = match *value {
                "on" => Some(Some(true)),
                "off" => Some(Some(false)),
                "inherit" => Some(None),
                _ => None,
            };
            value.map_or(Command::Invalid, |enabled| {
                Command::SetClass((*class).to_owned(), enabled)
            })
        }
        ["/settings", "quiet-hours", "inherit"] => Command::SetQuiet(QuietPolicy::Inherit),
        ["/settings", "quiet-hours", "disabled"] => Command::SetQuiet(QuietPolicy::Disabled),
        ["/settings", "quiet-hours", window] => {
            parse_window(window).map_or(Command::Invalid, Command::SetQuiet)
        }
        ["/settings", "high-priority-bypass", value] => {
            bool_value(value).map_or(Command::Invalid, Command::SetBypass)
        }
        _ => Command::Invalid,
    }
}

fn bool_value(value: &str) -> Option<bool> {
    match value {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

fn parse_window(value: &str) -> Option<QuietPolicy> {
    let (start, end) = value.split_once('-')?;
    let start = parse_minute(start)?;
    let end = parse_minute(end)?;
    (start != end).then_some(QuietPolicy::Custom {
        start_minute: start,
        end_minute: end,
    })
}

fn parse_minute(value: &str) -> Option<u16> {
    let (hour, minute) = value.split_once(':')?;
    if hour.len() != 2 || minute.len() != 2 {
        return None;
    }
    let hour: u16 = hour.parse().ok()?;
    let minute: u16 = minute.parse().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

fn render(preference: &NotificationPreferences) -> String {
    let global = if preference.enabled { "on" } else { "off" };
    let bypass = if preference.high_priority_bypass {
        "on"
    } else {
        "off"
    };
    let quiet = match preference.quiet_policy {
        QuietPolicy::Disabled => "disabled".to_owned(),
        QuietPolicy::Inherit => "inherit".to_owned(),
        QuietPolicy::Custom {
            start_minute,
            end_minute,
        } => format!(
            "{:02}:{:02}-{:02}:{:02} UTC",
            start_minute / 60,
            start_minute % 60,
            end_minute / 60,
            end_minute % 60
        ),
    };
    let classes = KNOWN_CLASSES
        .iter()
        .map(|class| {
            let value = match preference.class_enabled(class) {
                Some(true) => "on",
                Some(false) => "off",
                None => "inherit",
            };
            format!("{class}: {value}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<b>Notification settings</b>\nnotifications: {global}\nquiet-hours: {quiet}\nhigh-priority-bypass: {bypass}\n{classes}"
    )
}

fn usage() -> String {
    "<b>Notification settings</b>\nUse /settings, /settings notifications on|off, /settings notification &lt;class&gt; on|off|inherit, /settings quiet-hours inherit|disabled|HH:MM-HH:MM, or /settings high-priority-bypass on|off.".to_owned()
}

async fn enqueue(
    database: &Database,
    bot_id: i64,
    chat_id: i64,
    text: String,
    now: i64,
) -> Result<(), PersistenceError> {
    let payload = telegram_persistence::outbound_jobs::MessagePayload {
        text,
        parse_mode: Some("HTML".to_owned()),
        reply_markup: None,
    };
    let content_hash = payload.canonical()?;
    database
        .enqueue_outbound_job(
            &telegram_persistence::outbound_jobs::NewOutboundJob {
                bot_id,
                chat_id,
                kind: telegram_persistence::outbound_jobs::OutboundJobKind::SendMessage,
                payload,
                content_hash,
                operation_id: None,
                revision: None,
                correlation_id: None,
                next_attempt_at: None,
            },
            now,
        )
        .await
        .map(|_| ())
}
