//! Message text to capture intent: the pure grammar between a Telegram message and Platform
//! work, plus the deterministic idempotency keys that make double-sends converge.
//!
//! Grammar: the entire trimmed text is one http(s) URL, or `/summarize <url>` with exactly one
//! argument. Normalization is deliberately minimal - trim, then let the URL parser lowercase
//! scheme and host - so two spellings of one address share an operation and nothing else does.

use url::Url;

/// The longest address this flow accepts, matching Platform's own ceiling.
pub(crate) const MAX_URL_CHARS: usize = 2048;

/// A parsed capture intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaptureIntent {
    /// The normalized https URL to capture.
    pub url: String,
}

/// Opaque deep-link value carried by an exact Telegram `/start` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeepLinkToken(String);

impl DeepLinkToken {
    /// Borrow the opaque value for registry presentation.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Parse an exact opaque deep-link transport command.
pub(crate) fn parse_start_token(text: &str) -> Option<DeepLinkToken> {
    let token = text.trim().strip_prefix("/start ")?;
    if token.len() != 64
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(DeepLinkToken(token.to_owned()))
}

/// One capture submission with the idempotency key that describes it.
///
/// The key is `hex(sha256("capture.v2|" || sender || "|" || route path || "|" || body bytes))`
/// over [`platform_api::CaptureSubmission::request_parts`], the very value the Platform client
/// sends, so a key can never describe a body other than the one posted (XR-021 CONTRACTS R4-08).
/// The same body replays its original Platform operation; a different body (the same file
/// forwarded, another media type) is a separate operation over the same content.
#[must_use]
pub(crate) fn capture_submission(
    telegram_user_id: i64,
    source: platform_api::CaptureSource,
    origin: Option<serde_json::Value>,
) -> platform_api::CaptureSubmission {
    let unkeyed = platform_api::CaptureSubmission {
        idempotency_key: String::new(),
        source,
        origin,
    };
    platform_api::CaptureSubmission {
        idempotency_key: derive_key(telegram_user_id, &unkeyed, None),
        ..unkeyed
    }
}

/// The deterministic key for a deliberate retry of one FAILED operation: the submission's own key
/// derivation, salted with that operation's identifier so Platform creates a fresh operation
/// instead of replaying it. The unit tests in this module pin the derivation; the production
/// consumer arrives with the callback-token item.
#[cfg(test)]
#[must_use]
pub(crate) fn retry_key(
    telegram_user_id: i64,
    submission: &platform_api::CaptureSubmission,
    failed_operation_id: uuid::Uuid,
) -> String {
    derive_key(telegram_user_id, submission, Some(failed_operation_id))
}

/// The one derivation: `capture.v2`, the sender, the route path and the serialised body, plus the
/// failed operation for a retry. Only the request parts feed it, never the stored key.
fn derive_key(
    telegram_user_id: i64,
    submission: &platform_api::CaptureSubmission,
    retry_of: Option<uuid::Uuid>,
) -> String {
    let (path, body) = submission.request_parts();
    // A `serde_json::Value` always serialises: its object keys are strings.
    let body = serde_json::to_string(&body).unwrap_or_default();
    let salt = retry_of.map_or_else(String::new, |failed| format!("|retry:{failed}"));
    digest(&format!(
        "capture.v2|{telegram_user_id}|{path}|{body}{salt}"
    ))
}

/// Lowercase hex SHA-256.
fn digest(input: &str) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let out = hasher.finalize();
    let mut hex = String::with_capacity(out.len() * 2);
    for byte in out {
        use std::fmt::Write as _;
        // Writing two hex digits into a String cannot fail; the Result exists for formatters
        // backed by fallible sinks.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Parse message text into a capture intent, if it matches the grammar.
///
/// Returns `None` for anything that is not exactly one supported address - free text, other
/// commands, other schemes, over-length input - which callers settle as unsupported.
pub(crate) fn parse(text: &str) -> Option<CaptureIntent> {
    let trimmed = text.trim();
    let candidate = match trimmed.strip_prefix("/summarize") {
        Some(rest) => rest.trim(),
        None => trimmed,
    };
    if candidate.is_empty() || candidate.chars().count() > MAX_URL_CHARS {
        return None;
    }
    // One token only: an address smuggled into prose is free text, not an intent.
    if candidate.split_whitespace().count() != 1 {
        return None;
    }
    let parsed = Url::parse(candidate).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return None;
    }
    // The parser has lowercased scheme and host; serialization is the canonical spelling.
    Some(CaptureIntent {
        url: parsed.to_string(),
    })
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::expect_used,
    reason = "assertions inside the in-crate test module"
)]
mod tests {
    use super::{
        CaptureIntent, MAX_URL_CHARS, capture_submission, parse, parse_start_token, retry_key,
    };

    #[test]
    fn start_command_parses_only_a_64_character_opaque_token() {
        let valid = "A".repeat(64);
        assert_eq!(valid.len(), 64);
        assert_eq!(
            parse_start_token(&format!("/start {valid}")).map(|token| token.0),
            Some(valid),
        );

        for text in [
            "/start https://example.test/article",
            "/start {\"operation_id\":42}",
            "/start 123456789",
            "/start short",
            "/start AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "/start AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA extra",
            "/start",
        ] {
            assert!(parse_start_token(text).is_none(), "{text:?} must not parse");
        }
    }

    #[test]
    fn bare_https_url_and_summarize_form_parse_to_capture_intents() {
        let bare = parse("https://example.test/article").expect("a bare https URL parses");
        assert_eq!(
            bare,
            CaptureIntent {
                url: "https://example.test/article".to_owned()
            }
        );

        let command =
            parse("/summarize https://example.test/article").expect("the summarize form parses");
        assert_eq!(command, bare, "both forms derive the same intent");
    }

    #[test]
    fn host_and_scheme_case_normalize_to_one_canonical_url() {
        let upper = parse("HTTPS://Example.test/Article").expect("mixed case parses");
        assert_eq!(
            upper.url, "https://example.test/Article",
            "scheme and host lowercase; path case is content"
        );
    }

    #[test]
    fn non_http_schemes_free_text_and_missing_argument_do_not_parse() {
        for text in [
            "hello world",
            "/summarize",
            "/summarize   ",
            "/summarize ftp://example.test/x",
            "ftp://example.test/x",
            "/help",
            "read https://example.test/a later",
            "",
            "  ",
        ] {
            assert!(parse(text).is_none(), "{text:?} must not parse");
        }
    }

    #[test]
    fn urls_over_the_platform_limit_do_not_parse() {
        let long = format!("https://example.test/{}", "a".repeat(2100));
        assert!(parse(&long).is_none(), "over the ceiling must refuse");
        let prefix = "https://example.test/";
        let at_limit = format!("{prefix}{}", "a".repeat(MAX_URL_CHARS - prefix.len()));
        assert_eq!(at_limit.chars().count(), MAX_URL_CHARS);
        assert!(
            parse(&at_limit).is_some(),
            "exactly at the ceiling still parses"
        );
    }

    fn url_submission(
        sender: i64,
        url: &str,
        origin: Option<serde_json::Value>,
    ) -> platform_api::CaptureSubmission {
        capture_submission(
            sender,
            platform_api::CaptureSource::Url(url.to_owned()),
            origin,
        )
    }

    fn blob_submission(
        sender: i64,
        media_type: &str,
        length_bytes: u64,
        origin: Option<serde_json::Value>,
    ) -> platform_api::CaptureSubmission {
        capture_submission(
            sender,
            platform_api::CaptureSource::Blob {
                owner_service: "ratatoskr-telegram".to_owned(),
                digest_hex: "ab".repeat(32),
                media_type: media_type.to_owned(),
                length_bytes,
            },
            origin,
        )
    }

    fn forward(message_id: i64) -> serde_json::Value {
        serde_json::json!({
            "forward": {"kind": "channel", "chat_id": -100_200_300, "message_id": message_id,
                         "sent_at_secs": 1_700_000_000}
        })
    }

    #[test]
    fn keys_are_stable_across_repeats_and_normalizations() {
        let first =
            url_submission(900_700_601, "https://example.test/article", None).idempotency_key;
        let again =
            url_submission(900_700_601, "https://example.test/article", None).idempotency_key;
        assert_eq!(first, again, "the same sender and URL derive one key");
        assert_eq!(
            first,
            parse("HTTPS://EXAMPLE.test/article")
                .map(|intent| url_submission(900_700_601, &intent.url, None).idempotency_key)
                .expect("the variant parses"),
            "host and scheme case never changes the key"
        );
        assert_ne!(
            first,
            url_submission(900_700_602, "https://example.test/article", None).idempotency_key,
            "another sender derives another key"
        );
        assert_eq!(first.len(), 64, "the key is lowercase hex sha256");
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hex only, no uppercase"
        );
    }

    #[test]
    fn two_different_bodies_never_share_a_key_and_the_same_body_always_does() {
        let build: [fn() -> platform_api::CaptureSubmission; 9] = [
            || url_submission(900_700_601, "https://example.test/a", None),
            || url_submission(900_700_601, "https://example.test/a", Some(forward(77))),
            || url_submission(900_700_601, "https://example.test/a", Some(forward(78))),
            || url_submission(900_700_601, "https://example.test/b", None),
            || url_submission(900_700_602, "https://example.test/a", None),
            || blob_submission(900_700_601, "application/pdf", 1_024, None),
            || blob_submission(900_700_601, "application/pdf", 1_024, Some(forward(77))),
            || blob_submission(900_700_601, "application/x-pdf", 1_024, None),
            || blob_submission(900_700_601, "application/pdf", 2_048, None),
        ];
        let keys: Vec<String> = build.iter().map(|make| make().idempotency_key).collect();
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(key.len(), 64, "submission {index} has a sha256 key");
            assert_eq!(
                key,
                &build[index]().idempotency_key,
                "submission {index} derives one key every time"
            );
        }
        let distinct: std::collections::HashSet<&String> = keys.iter().collect();
        assert_eq!(
            distinct.len(),
            keys.len(),
            "no two bodies share a key: {keys:?}"
        );
    }

    /// The webhook only admits `application/pdf` today, so this is pinned on the derivation: the
    /// same bytes under another media type are another body, hence another operation.
    #[test]
    fn a_different_media_type_for_the_same_bytes_is_a_separate_operation() {
        let pdf = blob_submission(900_700_601, "application/pdf", 1_024, None);
        let other = blob_submission(900_700_601, "application/octet-stream", 1_024, None);
        assert_ne!(pdf.idempotency_key, other.idempotency_key);
        assert_eq!(
            pdf.idempotency_key,
            blob_submission(900_700_601, "application/pdf", 1_024, None).idempotency_key,
            "the same media type still converges"
        );
    }

    #[test]
    fn the_key_is_the_documented_digest_over_the_request_parts() {
        use sha2::{Digest as _, Sha256};
        let submission = url_submission(900_700_601, "https://example.test/a", Some(forward(77)));
        let (path, body) = submission.request_parts();
        let mut input = b"capture.v2|900700601|".to_vec();
        input.extend_from_slice(path.as_bytes());
        input.push(b'|');
        input.extend_from_slice(&serde_json::to_vec(&body).expect("the body serialises"));
        assert_eq!(
            submission.idempotency_key,
            format!("{:x}", Sha256::digest(&input)),
            "the key is sha256 over the versioned sender, path and body bytes"
        );
    }

    #[test]
    fn retry_keys_salt_with_the_failed_operation() {
        let submission = url_submission(900_700_601, "https://example.test/article", None);
        let failed = uuid::Uuid::now_v7();
        let retry = retry_key(900_700_601, &submission, failed);
        assert_eq!(retry.len(), 64, "a retry key is a sha256 hex digest");
        assert_ne!(
            submission.idempotency_key, retry,
            "a retry must not reuse the failed attempt's key"
        );
        assert_eq!(
            retry,
            retry_key(900_700_601, &submission, failed),
            "the same failed operation derives one stable retry key"
        );
        assert_ne!(
            retry,
            retry_key(900_700_601, &submission, uuid::Uuid::now_v7()),
            "a different failure salts differently"
        );
    }
}
