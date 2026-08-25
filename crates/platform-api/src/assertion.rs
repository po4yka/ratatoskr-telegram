//! Identity assertion issuance: how this service vouches for a Telegram sender on Platform.
//!
//! The wire format is fixed by Platform's verifier: a compact
//! `base64url(payload_json).base64url(ed25519_signature)` token whose payload carries exactly six
//! members — `issuer`, `subject`, `audience`, `nonce`, `issued_at`, `expires_at` — signed over the
//! encoded payload bytes. The signing key is configuration secret and lives only inside this
//! module's issuer type.

use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use uuid::Uuid;

use crate::PlatformError;

const URL_SAFE_NO_PAD: base64::engine::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The issuer name Platform believes. Mirrors the verifier's constant.
pub const TELEGRAM_ISSUER: &str = "ratatoskr-telegram";

/// What the token says: six members and no seventh, in the verifier's exact shape.
#[derive(Serialize)]
struct ClaimsWire<'a> {
    issuer: &'a str,
    subject: &'a str,
    audience: &'a str,
    nonce: String,
    issued_at: Timestamp,
    expires_at: Timestamp,
}

/// Signs short-lived identity assertions for one audience.
#[derive(Debug)]
pub struct AssertionIssuer {
    signing: SigningKey,
    audience: String,
}

impl AssertionIssuer {
    /// Build an issuer from the configured 32-byte Ed25519 seed and the Platform audience.
    ///
    /// # Errors
    ///
    /// Returns an error if the seed cannot form a signing key, which a validated configuration
    /// makes unreachable in production but tests exercise deliberately.
    pub fn from_seed(seed: &[u8; 32], audience: &str) -> Result<Self, PlatformError> {
        Ok(Self {
            signing: SigningKey::from_bytes(seed),
            audience: audience.to_owned(),
        })
    }

    /// Sign one assertion for `subject`, valid from `now` for `ttl`.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] if the claims cannot be encoded or signed.
    pub fn issue(
        &self,
        subject: &str,
        now: Timestamp,
        ttl: Duration,
    ) -> Result<String, PlatformError> {
        let lifetime = SignedDuration::from_secs(
            i64::try_from(ttl.as_secs()).map_err(|_| PlatformError::MalformedFrame)?,
        );
        let claims = ClaimsWire {
            issuer: TELEGRAM_ISSUER,
            subject,
            audience: &self.audience,
            // A UUIDv7 string is 36 characters: inside the store's 16..=128 bound.
            nonce: Uuid::now_v7().to_string(),
            issued_at: now,
            expires_at: now + lifetime,
        };
        let encoded = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).map_err(|_| PlatformError::MalformedFrame)?);
        let signature = self.signing.sign(encoded.as_bytes());
        Ok(format!(
            "{encoded}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }
}

/// Decode one base64url token part.
///
/// # Errors
///
/// [`PlatformError`] when the part is not valid base64url.
pub fn decode_part(part: &str) -> Result<Vec<u8>, PlatformError> {
    URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| PlatformError::MalformedFrame)
}
