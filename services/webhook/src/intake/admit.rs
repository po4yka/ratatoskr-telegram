//! The admission order, the capped body reader, and the queue handoff.
//!
//! One function owns the order so a reviewer can check it against AGENTS.md line by line:
//! secret before anything; method and content type before size; size before parse; parse before
//! dedupe; the dedupe insert atomic with acceptance; queue capacity reserved BEFORE the insert so
//! a saturated queue never leaves an accepted-but-never-processed row.

use axum::body::Body;
use http::request::Parts;
use http::{HeaderMap, Method, header};
use http_body_util::BodyExt as _;
use secrecy::ExposeSecret as _;
use subtle::ConstantTimeEq;
use telegram_persistence::{AdmittedUpdate, RecordOutcome};
use telegram_telemetry::metrics::TELEGRAM_UPDATES_RECEIVED_TOTAL;

use crate::intake::{Intake, Outcome, classify::kind_label};

/// The header Telegram echoes the configured secret in.
const SECRET_HEADER: &str = "x-telegram-bot-api-secret-token";

/// An update handed to asynchronous processing after its dedupe row was committed.
#[derive(Debug)]
pub struct QueuedUpdate {
    /// The bot that received it — half of every downstream state transition's key.
    pub bot_id: i64,
    /// The parsed update itself.
    pub update: bot_api::Update,
}

/// Constant-time secret comparison: equal bytes only when equal lengths, no early exit on
/// content. A length mismatch cannot be constant-time (the answer is determined by length alone),
/// which leaks at most what the attacker already sent.
pub(crate) fn secret_matches(expected: &secrecy::SecretString, provided: &[u8]) -> bool {
    let expected_bytes = expected.expose_secret().as_bytes();
    if provided.len() != expected_bytes.len() {
        return false;
    }
    bool::from(provided.ct_eq(expected_bytes))
}

/// The admission order. Every early return is an [`Outcome`]; only the last lines touch state.
pub(crate) async fn admit_ordered(intake: &Intake, parts: &Parts, body: Body) -> Outcome {
    // 1. Authenticity before anything else: no body byte is read or parsed for an unverified
    //    caller. A non-ASCII header value is not a secret we configured.
    let Some(provided) = secret_header(&parts.headers) else {
        return Outcome::Unauthorized;
    };
    if !secret_matches(&intake.settings.secret, provided.as_bytes()) {
        return Outcome::Unauthorized;
    }

    // 2. Method and content type are free to check and cheap to reject.
    if parts.method != Method::POST {
        return Outcome::MethodNotAllowed;
    }
    if !is_json(parts.headers.get(header::CONTENT_TYPE)) {
        return Outcome::WrongMediaType;
    }

    // 3. Declared size first, so an oversized delivery buys nothing.
    if declared_too_large(&parts.headers, intake.settings.max_body_bytes) {
        return Outcome::TooLarge;
    }

    // 4. Read with a hard cap: chunked bodies cannot lie about their length.
    let bytes = match read_body_capped(body, intake.settings.max_body_bytes).await {
        Ok(bytes) => bytes,
        Err(BodyTooLarge) => return Outcome::TooLarge,
    };

    // 5. Parse against the Bot API schema. Malformed is ACKED, not rejected: retrying a body that
    //    will never parse is how a webhook becomes a retry storm.
    let update = match serde_json::from_slice::<bot_api::Update>(&bytes) {
        Ok(update) => update,
        Err(error) => {
            tracing::info!(
                class = "malformed_update",
                error_class = parse_error_class(&error),
                "a delivery could not be parsed as a Bot API update",
            );
            return Outcome::Malformed;
        }
    };

    // 6. Record and hand off. The capacity reservation happens BEFORE the insert: if the queue has
    //    no room, nothing was persisted and 503 lets Telegram try again later.
    let kind = kind_label(&update.kind);
    let payload = match serde_json::to_string(&update) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::error!(
                error = %error,
                class = "serialization_failure",
                "the parsed update could not be stored",
            );
            return Outcome::Overloaded;
        }
    };
    metrics::counter!(TELEGRAM_UPDATES_RECEIVED_TOTAL, "update_kind" => kind).increment(1);
    match intake.sender.try_reserve() {
        Ok(permit) => {
            let admitted = AdmittedUpdate {
                bot_id: intake.settings.bot_id,
                update_id: i64::from(update.id.0),
                kind: kind.to_owned(),
                payload,
            };
            match intake.database.record_update(&admitted).await {
                Ok(RecordOutcome::Inserted) => {
                    permit.send(QueuedUpdate {
                        bot_id: intake.settings.bot_id,
                        update,
                    });
                    Outcome::Accepted
                }
                Ok(RecordOutcome::Duplicate) => Outcome::Deduplicated,
                Err(error) => {
                    drop(permit);
                    tracing::error!(
                        error = %error,
                        class = "storage_failure",
                        "the dedupe insert failed; the delivery is not acknowledged as accepted",
                    );
                    Outcome::Overloaded
                }
            }
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {
            tracing::warn!(class = "queue_saturated", "the processing queue is full");
            Outcome::Overloaded
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => {
            tracing::error!(class = "queue_closed", "the processing queue is closed");
            Outcome::Overloaded
        }
    }
}

/// The response body per outcome. Telegram ignores it; operators curling the endpoint do not.
pub(crate) fn response_body(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::TooLarge => "request body exceeds the configured limit\n",
        Outcome::Overloaded => "temporarily unable to accept updates; retry later\n",
        _ => "",
    }
}

/// The secret header value, if it is ASCII text.
fn secret_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(SECRET_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// Whether the content type names JSON. Parameters (`charset=...`) are ignored; a POST without a
/// content type is not JSON.
fn is_json(content_type: Option<&http::HeaderValue>) -> bool {
    content_type
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("application/json")
            })
        })
}

/// Whether a DECLARED content length already exceeds the cap.
fn declared_too_large(headers: &HeaderMap, max_body_bytes: usize) -> bool {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|declared| declared > max_body_bytes)
}

/// Reading past the cap fails instead of allocating it.
#[derive(Debug)]
pub struct BodyTooLarge;

/// Read a request body up to `max_bytes`, refusing one byte more.
///
/// Frames are summed as they arrive, so neither a lying `Content-Length` nor a chunked stream can
/// turn a forged delivery into allocated memory.
///
/// # Errors
///
/// Returns [`BodyTooLarge`] when the body exceeds `max_bytes` or a body frame cannot be read.
pub async fn read_body_capped(
    mut body: Body,
    max_bytes: usize,
) -> Result<axum::body::Bytes, BodyTooLarge> {
    let mut collected: Vec<u8> = Vec::with_capacity(max_bytes.min(65_536));
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| BodyTooLarge)?;
        let data = frame.into_data().map_err(|_| BodyTooLarge)?;
        if collected.len() + data.len() > max_bytes {
            return Err(BodyTooLarge);
        }
        collected.extend_from_slice(&data);
    }
    Ok(axum::body::Bytes::from(collected))
}

/// The safe failure class of a parse error, for logs that name classes and never bodies.
fn parse_error_class(error: &serde_json::Error) -> &'static str {
    use serde_json::error::Category;
    match error.classify() {
        Category::Syntax => "json_syntax",
        Category::Data => "schema_mismatch",
        Category::Eof => "truncated",
        Category::Io => "unreadable",
    }
}

#[cfg(test)]
mod tests {
    use super::secret_matches;
    use secrecy::SecretString;

    #[test]
    fn the_secret_comparison_accepts_only_exact_equal_lengths() {
        let secret = SecretString::new("webhook-secret-0123456789abcdef".into());
        assert!(secret_matches(&secret, b"webhook-secret-0123456789abcdef"));
        assert!(!secret_matches(&secret, b"webhook-secret-0123456789abcdeg"));
        assert!(!secret_matches(&secret, b"webhook-secret-0123456789abcde"));
        assert!(!secret_matches(
            &secret,
            b"webhook-secret-0123456789abcdefX"
        ));
    }
}
