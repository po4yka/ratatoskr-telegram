//! Assertion issuance, exercised against the exact shape Platform's verifier accepts.
//!
//! The token format is a cross-service contract: `base64url(payload).base64url(signature)`,
//! signed Ed25519 over the encoded payload bytes, six claim members and no seventh. These tests
//! pin that byte-for-byte so a drift breaks here rather than at an exchange route.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use ed25519_dalek::{SigningKey, Verifier as _, VerifyingKey};
use platform_api::assertion::AssertionIssuer;
use serde_json::Value;
use std::time::Duration;

/// A synthetic keypair seed. Never a production credential; fixed so signatures are stable.
const SEED: [u8; 32] = [
    9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 9, 8,
];

const AUDIENCE: &str = "ratatoskr-edge";
const SUBJECT: &str = "900700601";

/// The compact assertion carries exactly the six documented members, names the known issuer,
/// binds the requested subject and audience, carries a usable nonce, and verifies under the
/// paired public key over the encoded payload bytes.
#[test]
fn assertion_matches_the_verifier_shape() {
    let issuer = AssertionIssuer::from_seed(&SEED, AUDIENCE).expect("issuer builds");
    let now = jiff::Timestamp::constant(1_786_960_800, 0);
    let ttl = Duration::from_mins(5);

    let token = issuer
        .issue(SUBJECT, now, ttl)
        .expect("issuance must succeed");

    let (payload_b64, signature_b64) = token.split_once('.').expect("two base64url parts");
    let payload =
        platform_api::assertion::decode_part(payload_b64).expect("the payload part decodes");
    let signature_bytes =
        platform_api::assertion::decode_part(signature_b64).expect("the signature part decodes");

    // The signature covers the ENCODED payload exactly as it appears in the token — the same
    // bytes Platform's verifier feeds its Ed25519 check — not a re-serialization of the claims.
    let verifying: VerifyingKey = SigningKey::from_bytes(&SEED).verifying_key();
    let signature = ed25519_dalek::Signature::from_slice(&signature_bytes)
        .expect("an Ed25519 signature is 64 bytes");
    verifying
        .verify(payload_b64.as_bytes(), &signature)
        .expect("the signature must verify under the paired public key");

    let claims: Value = serde_json::from_slice(&payload).expect("claims parse");
    let object = claims.as_object().expect("claims are one JSON object");
    assert_eq!(
        object.len(),
        6,
        "exactly six members, no seventh: {object:?}"
    );
    assert_eq!(object["issuer"], "ratatoskr-telegram");
    assert_eq!(object["subject"], SUBJECT);
    assert_eq!(object["audience"], AUDIENCE);
    let nonce = object["nonce"].as_str().expect("nonce is text");
    assert!(
        (16..=128).contains(&nonce.len()),
        "the nonce must fit the store's bound, got {nonce}"
    );
    assert_eq!(
        object["issued_at"],
        Value::String("2026-08-17T10:00:00Z".to_owned()),
        "issued_at serializes as the instant given"
    );
    assert_eq!(
        object["expires_at"],
        Value::String("2026-08-17T10:05:00Z".to_owned()),
        "expires_at is issued_at plus the lifetime"
    );
}
