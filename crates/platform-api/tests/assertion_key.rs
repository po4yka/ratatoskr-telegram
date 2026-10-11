//! The assertion key: its public half must be the half of the key the signer uses
//! (XR-021 CONTRACTS.md section R3-12).

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use platform_api::assertion::AssertionIssuer;
use platform_api::assertion_key::{decode_seed, generate, public_key_base64};
use std::time::Duration;

/// RFC 8032 section 7.1, test 1.
const VECTOR_SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const VECTOR_PUBLIC_BASE64: &str = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=";

#[test]
fn the_public_key_matches_the_signer() {
    let public = public_key_base64(VECTOR_SEED).expect("a valid seed");
    assert_eq!(public, VECTOR_PUBLIC_BASE64);

    let seed = decode_seed(VECTOR_SEED).expect("a valid seed");
    let issuer = AssertionIssuer::from_seed(&seed, "edge").expect("issuer");
    let token = issuer
        .issue(
            "900700601",
            jiff::Timestamp::constant(1_786_960_800, 0),
            Duration::from_mins(5),
        )
        .expect("an assertion");

    let key_bytes: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(public)
        .expect("standard padded base64")
        .try_into()
        .expect("32 bytes");
    let verifying = VerifyingKey::from_bytes(&key_bytes).expect("a public key");
    let (payload, signature) = token.split_once('.').expect("two parts");
    let signature = Signature::from_slice(
        &platform_api::assertion::decode_part(signature).expect("signature part"),
    )
    .expect("64 bytes");
    verifying
        .verify(payload.as_bytes(), &signature)
        .expect("the printed key verifies what the signer signs");
}

#[test]
fn decode_accepts_both_hex_cases_and_refuses_everything_else() {
    assert_eq!(
        decode_seed(&VECTOR_SEED.to_uppercase()).expect("upper case"),
        decode_seed(VECTOR_SEED).expect("lower case"),
    );
    for bad in ["", "abcd", &"g".repeat(64), &format!("{VECTOR_SEED}00")] {
        let error = decode_seed(bad).expect_err("malformed");
        assert!(
            !format!("{error} {error:?}").contains(bad) || bad.is_empty(),
            "the error echoed the seed",
        );
    }
}

#[test]
fn generate_creates_a_private_seed_file_and_never_overwrites_it() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("seed");
    let public = generate(&path).expect("a new seed");

    let content = std::fs::read_to_string(&path).expect("the seed file");
    assert_eq!(content.len(), 65);
    assert!(content.ends_with('\n'));
    assert!(
        content[..64]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "64 lowercase hex characters",
    );
    assert_eq!(
        std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        public_key_base64(content.trim_end()).expect("derives"),
        public
    );

    generate(&path).expect_err("an existing file is refused");
    assert_eq!(std::fs::read_to_string(&path).expect("unchanged"), content);
}
