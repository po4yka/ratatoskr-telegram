//! Update payload parsing against recorded synthetic fixtures.
//!
//! The envelope is `(update_id, one kind key)`. A well-formed envelope with a kind this build
//! does not know parses as unrecognized; anything else fails. These fixtures are synthetic: no
//! real chat, user, or message content.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use bot_api::UpdateKind;

/// The message fixture parses with its update id and typed message kind.
#[test]
fn a_recorded_message_update_parses_with_its_id_and_kind() {
    let raw = include_str!("fixtures/message.json");
    let update: bot_api::Update = serde_json::from_str(raw).expect("the fixture must parse");
    assert_eq!(update.id.0, 1001);
    assert!(matches!(update.kind, UpdateKind::Message(_)));
}

/// The callback query fixture parses with its typed callback kind.
#[test]
fn a_recorded_callback_query_parses_with_its_kind() {
    let raw = include_str!("fixtures/callback_query.json");
    let update: bot_api::Update = serde_json::from_str(raw).expect("the fixture must parse");
    match &update.kind {
        UpdateKind::CallbackQuery(query) => {
            assert_eq!(query.id.0, "4382fecwq");
            assert_eq!(query.data.as_deref(), Some("opaque-intent-token"));
        }
        other => panic!("expected a callback query, got {other:?}"),
    }
}

/// An unknown kind inside a well-formed envelope is unsupported input, not malformed input:
/// parsing succeeds and the value reports itself as unrecognized.
#[test]
fn an_unknown_kind_is_unrecognized_not_malformed() {
    let raw = include_str!("fixtures/unknown_kind.json");
    let update: bot_api::Update =
        serde_json::from_str(raw).expect("a well-formed envelope must parse");
    assert_eq!(update.id.0, 1003);
    assert!(
        matches!(update.kind, UpdateKind::Error(_)),
        "{:?}",
        update.kind
    );
}

/// An envelope without `update_id` fails parsing — there is nothing to deduplicate on.
#[test]
fn an_envelope_without_update_id_fails() {
    let raw = include_str!("fixtures/missing_id.json");
    serde_json::from_str::<bot_api::Update>(raw)
        .expect_err("an envelope without an update id must fail to parse");
}

/// A body that is not JSON at all fails parsing.
#[test]
fn a_non_json_body_fails() {
    serde_json::from_str::<bot_api::Update>("<html>not json</html>")
        .expect_err("non-JSON must fail to parse");
}
