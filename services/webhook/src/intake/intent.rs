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

/// The deterministic idempotency key for a first capture attempt.
///
/// Hex-encoded SHA-256 over a versioned binding of sender, address, and intent kind, so the
/// same sender resending one address converges on Platform's original operation.
#[must_use]
pub(crate) fn capture_key(telegram_user_id: i64, normalized_url: &str) -> String {
    digest(&format!("capture.v1|{telegram_user_id}|{normalized_url}"))
}

/// The deterministic idempotency key for an attachment capture.
///
/// The source is the bytes' verified SHA-256 digest rather than a Bot API file identifier: those
/// identifiers are provider-local and can change, whereas identical content must converge on one
/// Platform operation for the sending Telegram user.
#[must_use]
pub(crate) fn blob_capture_key(telegram_user_id: i64, digest_hex: &str) -> String {
    digest(&format!(
        "capture.v1|{telegram_user_id}|sha256:{digest_hex}"
    ))
}

/// The deterministic key for a deliberate retry of one FAILED operation: salted with that
/// operation's identifier, so Platform creates a fresh operation instead of replaying it.
/// The unit tests in this module pin the derivation; the production consumer arrives with the
/// callback-token item.
#[cfg(test)]
#[must_use]
pub(crate) fn retry_key(
    telegram_user_id: i64,
    normalized_url: &str,
    failed_operation_id: uuid::Uuid,
) -> String {
    digest(&format!(
        "capture.v1|{telegram_user_id}|{normalized_url}|retry:{failed_operation_id}"
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
    use super::{CaptureIntent, MAX_URL_CHARS, capture_key, parse, retry_key};

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

    #[test]
    fn keys_are_stable_across_repeats_and_normalizations() {
        let first = capture_key(900_700_601, "https://example.test/article");
        let again = capture_key(900_700_601, "https://example.test/article");
        assert_eq!(first, again, "the same sender and URL derive one key");
        assert_eq!(
            first,
            parse("HTTPS://EXAMPLE.test/article")
                .map(|intent| capture_key(900_700_601, &intent.url))
                .expect("the variant parses"),
            "host and scheme case never changes the key"
        );
        assert_ne!(
            first,
            capture_key(900_700_602, "https://example.test/article"),
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
    fn retry_keys_salt_with_the_failed_operation() {
        let base = capture_key(900_700_601, "https://example.test/article");
        let failed = uuid::Uuid::now_v7();
        let retry = retry_key(900_700_601, "https://example.test/article", failed);
        assert_ne!(
            base, retry,
            "a retry must not reuse the failed attempt's key"
        );
        assert_eq!(
            retry,
            retry_key(900_700_601, "https://example.test/article", failed),
            "the same failed operation derives one stable retry key"
        );
        assert_ne!(
            retry,
            retry_key(
                900_700_601,
                "https://example.test/article",
                uuid::Uuid::now_v7()
            ),
            "a different failure salts differently"
        );
    }
}
