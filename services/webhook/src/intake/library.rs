//! Private-chat library command adapter over Platform's authenticated public API.

use secrecy::ExposeSecret as _;
use telegram_telemetry::metrics::TELEGRAM_LIBRARY_COMMANDS_TOTAL;

use super::worker::{CaptureContext, enqueue_text, now_secs};

const SEARCH_CAPABILITY: &str = "library.search";
const READ_CAPABILITY: &str = "library.read_state";
const PAGE_SIZE: u32 = 5;
const MAX_SEARCH_CHARS: usize = 256;
const READ_TOKEN_TTL_SECS: i64 = 15 * 60;
const MAX_TITLE_CHARS: usize = 160;
const MAX_SNIPPET_CHARS: usize = 320;
const HELP_BODY: &str = "<b>Ratatoskr commands</b>\n<code>/search &lt;query&gt;</code> — search the library (1–256 characters)\n<code>/unread</code> — show up to five unread items\n<code>/read &lt;token&gt;</code> — mark an offered item read; tokens expire after 15 minutes\n<code>/settings</code> — notification preferences";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Help,
    Search(String),
    Unread,
    Read(String),
    Invalid(CommandClass, &'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandClass {
    Search,
    Unread,
    Read,
}

impl CommandClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Unread => "unread",
            Self::Read => "read",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Outcome {
    Usage,
    FeatureUnavailable,
    Timeout,
    Empty,
    Results,
    Success,
    Expired,
    NotFound,
    Unknown,
    StorageError,
}

impl Outcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::FeatureUnavailable => "feature_unavailable",
            Self::Timeout => "timeout",
            Self::Empty => "empty",
            Self::Results => "results",
            Self::Success => "success",
            Self::Expired => "expired",
            Self::NotFound => "not_found",
            Self::Unknown => "unknown",
            Self::StorageError => "storage_error",
        }
    }
}

/// Handle a library-shaped command, or return `None` when another adapter should inspect it.
pub(super) async fn handle(
    database: &telegram_persistence::Database,
    bot_id: i64,
    telegram_user_id: i64,
    chat_id: i64,
    text: &str,
    context: Option<&CaptureContext>,
) -> Option<telegram_persistence::UpdateState> {
    let command = parse(text)?;
    if command == Command::Help {
        return Some(enqueue_text(database, bot_id, chat_id, HELP_BODY).await);
    }
    let Some(context) = context else {
        return Some(telegram_persistence::UpdateState::Processed);
    };
    let result = match command {
        Command::Help => unreachable!("help returns before Platform context is required"),
        Command::Invalid(command, usage) => {
            observed_reply(database, bot_id, chat_id, command, Outcome::Usage, usage).await
        }
        Command::Search(query) => {
            run_query(
                database,
                bot_id,
                chat_id,
                telegram_user_id,
                context,
                query,
                None,
            )
            .await
        }
        Command::Unread => {
            run_query(
                database,
                bot_id,
                chat_id,
                telegram_user_id,
                context,
                String::new(),
                Some(platform_api::LibraryReadState::Unread),
            )
            .await
        }
        Command::Read(token) => {
            run_read(database, bot_id, telegram_user_id, chat_id, context, &token).await
        }
    };
    Some(result)
}

async fn run_query(
    database: &telegram_persistence::Database,
    bot_id: i64,
    chat_id: i64,
    telegram_user_id: i64,
    context: &CaptureContext,
    query: String,
    read_state: Option<platform_api::LibraryReadState>,
) -> telegram_persistence::UpdateState {
    let subject = telegram_user_id.to_string();
    let command = if read_state == Some(platform_api::LibraryReadState::Unread) {
        CommandClass::Unread
    } else {
        CommandClass::Search
    };
    let Ok(session) = context.sessions.session(&subject).await else {
        return unavailable(database, bot_id, chat_id, command).await;
    };
    let credential = session.credential.expose_secret();
    let Ok(capabilities) = context.sessions.client().capabilities(credential).await else {
        return unavailable(database, bot_id, chat_id, command).await;
    };
    if !capabilities.contains(SEARCH_CAPABILITY) {
        return unavailable(database, bot_id, chat_id, command).await;
    }
    let read_available = capabilities.contains(READ_CAPABILITY);
    let unread = read_state == Some(platform_api::LibraryReadState::Unread);
    let page = match context
        .sessions
        .client()
        .search_library(
            credential,
            &platform_api::LibrarySearch {
                query,
                read_state,
                limit: PAGE_SIZE,
                offset: 0,
            },
        )
        .await
    {
        Ok(page) => page,
        Err(platform_api::PlatformError::Timeout) => {
            return observed_reply(
                database,
                bot_id,
                chat_id,
                command,
                Outcome::Timeout,
                "Library search is temporarily unavailable.",
            )
            .await;
        }
        Err(_) => return unavailable(database, bot_id, chat_id, command).await,
    };
    let empty = page.items.is_empty();
    let state = enqueue_page(
        database,
        telegram_persistence::interaction_tokens::LibraryReadScope {
            bot_id,
            telegram_user_id,
            internal_user_id: session.user_id,
            chat_id,
        },
        &page,
        unread,
        read_available,
    )
    .await;
    observe(
        command,
        if state == telegram_persistence::UpdateState::Failed {
            Outcome::StorageError
        } else if empty {
            Outcome::Empty
        } else {
            Outcome::Results
        },
    );
    state
}

fn parse(text: &str) -> Option<Command> {
    let trimmed = text.trim();
    if trimmed == "/help" {
        return Some(Command::Help);
    }
    if let Some(rest) = trimmed.strip_prefix("/search") {
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let query = rest.trim();
        return Some(if (1..=MAX_SEARCH_CHARS).contains(&query.chars().count()) {
            Command::Search(query.to_owned())
        } else {
            Command::Invalid(
                CommandClass::Search,
                "Usage: <code>/search &lt;query&gt;</code>",
            )
        });
    }
    if let Some(rest) = trimmed.strip_prefix("/unread") {
        return Some(if rest.is_empty() {
            Command::Unread
        } else if rest.starts_with(char::is_whitespace) {
            Command::Invalid(CommandClass::Unread, "Usage: <code>/unread</code>")
        } else {
            return None;
        });
    }
    if let Some(rest) = trimmed.strip_prefix("/read") {
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let token = rest.trim();
        return Some(if valid_token(token) {
            Command::Read(token.to_owned())
        } else {
            Command::Invalid(
                CommandClass::Read,
                "Usage: <code>/read &lt;token&gt;</code>",
            )
        });
    }
    None
}

fn valid_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

async fn enqueue_page(
    database: &telegram_persistence::Database,
    scope: telegram_persistence::interaction_tokens::LibraryReadScope,
    page: &platform_api::LibraryPage,
    unread_filter: bool,
    read_available: bool,
) -> telegram_persistence::UpdateState {
    if page.items.is_empty() {
        let body = if unread_filter {
            "No unread items.".to_owned()
        } else {
            "No results.".to_owned()
        };
        return enqueue_text(database, scope.bot_id, scope.chat_id, &body).await;
    }
    let now = now_secs();
    let intents: Vec<_> = page
        .items
        .iter()
        .take(PAGE_SIZE as usize)
        .filter(|item| read_available && item.read_state == platform_api::LibraryReadState::Unread)
        .map(|item| {
            telegram_persistence::interaction_tokens::PreparedLibraryReadIntent::new(
                telegram_persistence::interaction_tokens::NewLibraryReadIntent {
                    scope,
                    analysis_id: item.analysis_id,
                    expires_at: now + READ_TOKEN_TTL_SECS,
                },
            )
        })
        .collect();
    let Some(body) = render_page(page, &intents, read_available) else {
        return telegram_persistence::UpdateState::Failed;
    };
    let payload = telegram_persistence::outbound_jobs::MessagePayload {
        text: body,
        parse_mode: Some("HTML".to_owned()),
        reply_markup: None,
    };
    let Ok(content_hash) = payload.canonical() else {
        return telegram_persistence::UpdateState::Failed;
    };
    let job = telegram_persistence::outbound_jobs::NewOutboundJob {
        bot_id: scope.bot_id,
        chat_id: scope.chat_id,
        kind: telegram_persistence::outbound_jobs::OutboundJobKind::SendMessage,
        payload,
        content_hash,
        operation_id: None,
        revision: None,
        correlation_id: None,
        next_attempt_at: None,
    };
    match database
        .enqueue_library_read_delivery(&intents, &job, now)
        .await
    {
        Ok(_) => telegram_persistence::UpdateState::Processed,
        Err(_) => telegram_persistence::UpdateState::Failed,
    }
}

fn render_page(
    page: &platform_api::LibraryPage,
    intents: &[telegram_persistence::interaction_tokens::PreparedLibraryReadIntent],
    read_available: bool,
) -> Option<String> {
    let mut body = String::from("<b>Library</b>");
    let mut unread_intents = intents.iter();
    for item in page.items.iter().take(PAGE_SIZE as usize) {
        body.push_str("\n\n");
        body.push_str(&escape_bounded(&item.title, MAX_TITLE_CHARS));
        body.push_str(match item.read_state {
            platform_api::LibraryReadState::Unread => "\n<i>unread</i>",
            platform_api::LibraryReadState::Read => "\n<i>read</i>",
        });
        if let Some(snippet) = &item.snippet {
            body.push('\n');
            body.push_str(&escape_bounded(snippet, MAX_SNIPPET_CHARS));
        }
        if read_available && item.read_state == platform_api::LibraryReadState::Unread {
            let token = unread_intents.next()?.token();
            body.push_str("\n<code>/read ");
            body.push_str(token);
            body.push_str("</code>");
        }
    }
    debug_assert!(body.chars().count() < 4096);
    Some(body)
}

async fn run_read(
    database: &telegram_persistence::Database,
    bot_id: i64,
    telegram_user_id: i64,
    chat_id: i64,
    context: &CaptureContext,
    token: &str,
) -> telegram_persistence::UpdateState {
    let subject = telegram_user_id.to_string();
    let Ok(session) = context.sessions.session(&subject).await else {
        return read_unavailable(database, bot_id, chat_id).await;
    };
    let credential = session.credential.expose_secret();
    let Ok(capabilities) = context.sessions.client().capabilities(credential).await else {
        return read_unavailable(database, bot_id, chat_id).await;
    };
    if !capabilities.contains(READ_CAPABILITY) {
        return read_unavailable(database, bot_id, chat_id).await;
    }
    let presentation = telegram_persistence::interaction_tokens::LibraryReadPresentation {
        token,
        scope: telegram_persistence::interaction_tokens::LibraryReadScope {
            bot_id,
            telegram_user_id,
            internal_user_id: session.user_id,
            chat_id,
        },
        now: now_secs(),
    };
    let released = match database.resolve_library_read_intent(presentation).await {
        Ok(Ok(released)) => released,
        Ok(Err(_)) => {
            return observed_reply(
                database,
                bot_id,
                chat_id,
                CommandClass::Read,
                Outcome::Expired,
                "This read action has expired. Use <code>/unread</code> to refresh.",
            )
            .await;
        }
        Err(_) => return read_unavailable(database, bot_id, chat_id).await,
    };

    let mut uncertain = false;
    for _ in 0..2 {
        match context
            .sessions
            .client()
            .replace_library_read_state(
                credential,
                released.analysis_id,
                platform_api::LibraryReadState::Read,
            )
            .await
        {
            Ok(resource) if resource.read_state == platform_api::LibraryReadState::Read => {
                return observed_reply(
                    database,
                    bot_id,
                    chat_id,
                    CommandClass::Read,
                    Outcome::Success,
                    "Item marked as read.",
                )
                .await;
            }
            Ok(_) | Err(platform_api::PlatformError::Timeout) => uncertain = true,
            Err(platform_api::PlatformError::NotFound) => {
                return observed_reply(
                    database,
                    bot_id,
                    chat_id,
                    CommandClass::Read,
                    Outcome::NotFound,
                    "This library item is no longer available.",
                )
                .await;
            }
            Err(platform_api::PlatformError::Network(error)) => {
                uncertain |= !error.is_connect();
            }
            Err(
                platform_api::PlatformError::ServerError { .. }
                | platform_api::PlatformError::RateLimited,
            ) => {}
            Err(_) => return read_unavailable(database, bot_id, chat_id).await,
        }
    }
    if uncertain {
        observed_reply(
            database,
            bot_id,
            chat_id,
            CommandClass::Read,
            Outcome::Unknown,
            "The read outcome is unknown. Use <code>/unread</code> to reconcile it.",
        )
        .await
    } else {
        read_unavailable(database, bot_id, chat_id).await
    }
}

async fn read_unavailable(
    database: &telegram_persistence::Database,
    bot_id: i64,
    chat_id: i64,
) -> telegram_persistence::UpdateState {
    observed_reply(
        database,
        bot_id,
        chat_id,
        CommandClass::Read,
        Outcome::FeatureUnavailable,
        "Library read state is temporarily unavailable.",
    )
    .await
}

fn escape_bounded(value: &str, limit: usize) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        let replacement = match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            _ => {
                if escaped.chars().count() == limit {
                    break;
                }
                escaped.push(character);
                continue;
            }
        };
        if escaped.chars().count() + replacement.chars().count() > limit {
            break;
        }
        escaped.push_str(replacement);
    }
    escaped
}

async fn unavailable(
    database: &telegram_persistence::Database,
    bot_id: i64,
    chat_id: i64,
    command: CommandClass,
) -> telegram_persistence::UpdateState {
    observed_reply(
        database,
        bot_id,
        chat_id,
        command,
        Outcome::FeatureUnavailable,
        "Library search is temporarily unavailable.",
    )
    .await
}

async fn observed_reply(
    database: &telegram_persistence::Database,
    bot_id: i64,
    chat_id: i64,
    command: CommandClass,
    outcome: Outcome,
    text: &str,
) -> telegram_persistence::UpdateState {
    let state = enqueue_text(database, bot_id, chat_id, text).await;
    observe(
        command,
        if state == telegram_persistence::UpdateState::Failed {
            Outcome::StorageError
        } else {
            outcome
        },
    );
    state
}

fn observe(command: CommandClass, outcome: Outcome) {
    metrics::counter!(
        TELEGRAM_LIBRARY_COMMANDS_TOTAL,
        "command" => command.as_str(),
        "outcome" => outcome.as_str(),
    )
    .increment(1);
    tracing::info!(
        command = command.as_str(),
        outcome = outcome.as_str(),
        "library command settled",
    );
}

#[cfg(test)]
mod tests {
    use super::{Command, HELP_BODY, MAX_SEARCH_CHARS, PAGE_SIZE, READ_TOKEN_TTL_SECS, parse};

    #[test]
    fn exact_library_command_grammar() {
        assert_eq!(parse("/help"), Some(Command::Help));
        assert_eq!(
            parse("/search durable queues"),
            Some(Command::Search("durable queues".into()))
        );
        assert_eq!(parse("/unread"), Some(Command::Unread));
        assert!(matches!(parse("/search"), Some(Command::Invalid(..))));
        assert!(matches!(parse("/unread extra"), Some(Command::Invalid(..))));
        assert!(matches!(parse("/read short"), Some(Command::Invalid(..))));
        assert_eq!(parse("/searching"), None);
    }

    #[test]
    fn help_copy_names_exact_commands_and_static_bounds() {
        for command in ["/search &lt;query&gt;", "/unread", "/read &lt;token&gt;"] {
            assert!(HELP_BODY.contains(command));
        }
        assert!(HELP_BODY.contains(&MAX_SEARCH_CHARS.to_string()));
        assert!(HELP_BODY.contains(&PAGE_SIZE.to_string()));
        assert!(HELP_BODY.contains(&(READ_TOKEN_TTL_SECS / 60).to_string()));
    }
}
