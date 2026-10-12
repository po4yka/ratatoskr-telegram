//! A capture's idempotency key describes the body Platform fingerprints (XR-021 CONTRACTS R4-08
//! rule 1): the same message twice converges, and the same content sent in another shape is a
//! second accepted operation instead of a refusal.

use super::*;

/// The operation id the ledger mints for its `index`th distinct key. Index 0 is `OPERATION_ID`.
fn ledger_operation_id(index: usize) -> String {
    format!("018f0000-0000-7000-8000-{:012x}", 0xcafe + index)
}

/// The fake Platform's idempotency ledger. `Some(operation)` for a new key or a replay of the same
/// body; `None` for a key already bound to a different body, which Platform answers `409`.
pub(super) fn ledger_verdict(
    state: &PlatformState,
    headers: &axum::http::HeaderMap,
    body: &[u8],
) -> Option<String> {
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let mut ledger = state.ledger.lock().expect("ledger lock");
    match ledger
        .iter()
        .enumerate()
        .find(|(_, (known, _))| *known == key)
    {
        Some((index, (_, first_body))) if first_body == body => Some(ledger_operation_id(index)),
        Some(_) => None,
        None => {
            ledger.push((key, body.to_vec()));
            Some(ledger_operation_id(ledger.len() - 1))
        }
    }
}

/// A PDF document forwarded from a channel.
fn forwarded_pdf_update(update_id: i64, file_id: &str, file_size: u64) -> Value {
    let mut update = pdf_document_update(update_id, file_id, file_size);
    if let Some(message) = update.get_mut("message").and_then(Value::as_object_mut) {
        message.insert(
            "forward_origin".to_owned(),
            json!({
                "type": "channel",
                "chat": {"id": -100_200_300, "type": "channel", "title": "Synthetic Channel"},
                "message_id": 77,
                "date": 1_700_000_000_i64,
            }),
        );
    }
    update
}

/// The acknowledgments only: no failure reply of any kind.
fn is_acknowledgment(body: &str) -> bool {
    body.starts_with("<b>Capturing")
}

async fn assert_two_accepted_operations(
    fixture: &Fixture,
    state: &PlatformState,
    first_update: i64,
    second_update: i64,
) {
    assert_eq!(fixture.settled_state(first_update).await, "processed");
    assert_eq!(fixture.settled_state(second_update).await, "processed");
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 2);
    let keys = state.idempotency_keys.lock().expect("keys lock").clone();
    let [first, second] = keys.as_slice() else {
        panic!("two submissions carry two keys: {keys:?}");
    };
    assert_ne!(first, second, "another body is another key");
    let bodies = outbound_bodies(fixture).await;
    assert_eq!(bodies.len(), 2, "two acknowledgments: {bodies:?}");
    assert!(
        bodies.iter().all(|body| is_acknowledgment(body)),
        "no refusal or unavailable reply: {bodies:?}"
    );
    for index in 0..2 {
        let operation: uuid::Uuid = ledger_operation_id(index).parse().expect("a uuid");
        assert_eq!(
            binding_count(fixture, operation).await,
            1,
            "operation {index} is bound to its own acknowledgment"
        );
    }
}

/// The Round 3 smoke shape: a PDF sent plain (202) and then forwarded (same digest, another body)
/// used to answer 409 and the refusal reply although the first send was saved.
#[tokio::test]
async fn a_pdf_sent_plain_and_then_forwarded_yields_two_accepted_operations() {
    let (base_url, state) = platform_harness(CaptureAnswer::Ledger).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Ledger, Arc::clone(&state)).await;

    fixture
        .deliver(pdf_document_update(10_001, "pdf-plain", 128))
        .await;
    assert_eq!(fixture.settled_state(10_001).await, "processed");
    fixture
        .deliver(forwarded_pdf_update(10_002, "pdf-forwarded", 128))
        .await;

    assert_two_accepted_operations(&fixture, &state, 10_001, 10_002).await;
    assert_eq!(
        *state.capture_paths.lock().expect("path lock"),
        ["/v1/captures/blobs", "/v1/captures/blobs"]
    );
}

/// The same shape for a URL: sent bare, then forwarded from a channel.
#[tokio::test]
async fn a_url_sent_bare_and_then_forwarded_yields_two_accepted_operations() {
    let (base_url, state) = platform_harness(CaptureAnswer::Ledger).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Ledger, Arc::clone(&state)).await;

    fixture
        .deliver(message_update(10_011, "https://example.test/story"))
        .await;
    assert_eq!(fixture.settled_state(10_011).await, "processed");
    fixture
        .deliver(forwarded_update(10_012, "https://example.test/story"))
        .await;

    assert_two_accepted_operations(&fixture, &state, 10_011, 10_012).await;
}

/// A double send is one body, hence one key, hence one replayed operation and one acknowledgment.
#[tokio::test]
async fn the_same_message_sent_twice_converges_on_one_operation() {
    let (base_url, state) = platform_harness(CaptureAnswer::Ledger).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Ledger, Arc::clone(&state)).await;

    for update_id in [10_021, 10_022] {
        fixture
            .deliver(forwarded_update(update_id, "https://example.test/story"))
            .await;
        assert_eq!(fixture.settled_state(update_id).await, "processed");
    }
    fixture
        .deliver(pdf_document_update(10_023, "pdf-once", 128))
        .await;
    fixture
        .deliver(pdf_document_update(10_024, "pdf-twice", 128))
        .await;
    assert_eq!(fixture.settled_state(10_023).await, "processed");
    assert_eq!(fixture.settled_state(10_024).await, "processed");

    let keys = state.idempotency_keys.lock().expect("keys lock").clone();
    let [link, link_again, file, file_again] = keys.as_slice() else {
        panic!("four submissions carry four keys: {keys:?}");
    };
    assert_eq!(link, link_again, "the same forwarded link derives one key");
    assert_eq!(file, file_again, "the same file derives one key");
    assert_ne!(link, file);
    assert_eq!(state.ledger.lock().expect("ledger lock").len(), 2);
    assert_eq!(
        outbound_bodies(&fixture)
            .await
            .iter()
            .filter(|body| !is_acknowledgment(body))
            .count(),
        0,
        "a replay is never answered with a failure reply"
    );
    let operation: uuid::Uuid = ledger_operation_id(0).parse().expect("a uuid");
    assert_eq!(binding_count(&fixture, operation).await, 1);
}
