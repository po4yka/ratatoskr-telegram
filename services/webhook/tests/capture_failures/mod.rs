//! Every capture failure ends in one static reply, never in silence (XR-021 CONTRACTS S11).

use super::*;

/// A deliberately unreachable Platform: the session exchange itself fails at the transport level.
#[tokio::test]
async fn a_dead_platform_enqueues_a_temporarily_unavailable_reply() {
    let dead = dead_platform_url();
    let fixture = Fixture::create(&dead, CaptureAnswer::Accept, Arc::default()).await;
    fixture
        .deliver(message_update(9_401, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_401).await, "failed");
    assert_eq!(outbound_bodies(&fixture).await, [UNAVAILABLE_BODY]);
}

/// A reachable Platform answering `503` on both bounded attempts: transient classes exhausted.
#[tokio::test]
async fn a_platform_outage_enqueues_a_temporarily_unavailable_reply() {
    let (base_url, state) = platform_harness(CaptureAnswer::Unavailable).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Unavailable, Arc::clone(&state)).await;
    fixture
        .deliver(message_update(9_411, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_411).await, "failed");
    assert_eq!(
        state.capture_calls.load(Ordering::SeqCst),
        2,
        "the transient class is retried within its bound, then exhausted"
    );
    assert_eq!(outbound_bodies(&fixture).await, [UNAVAILABLE_BODY]);
    assert_eq!(
        binding_count(&fixture, OPERATION_ID.parse().unwrap()).await,
        0
    );
}

/// A permanent refusal settles after exactly one attempt and tells the sender the item was not
/// saved.
#[tokio::test]
async fn a_platform_permanent_refusal_enqueues_a_could_not_save_reply() {
    let (base_url, state) = platform_harness(CaptureAnswer::RefuseBadRequest).await;
    let fixture = Fixture::create(
        &base_url,
        CaptureAnswer::RefuseBadRequest,
        Arc::clone(&state),
    )
    .await;
    fixture
        .deliver(message_update(9_421, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_421).await, "failed");
    assert_eq!(
        state.capture_calls.load(Ordering::SeqCst),
        1,
        "a permanent class must not retry"
    );
    assert_eq!(outbound_bodies(&fixture).await, [REFUSED_BODY]);
}

/// A credential refusal is as permanent as any other refusal and gets the same static reply.
#[tokio::test]
async fn permanent_refusal_settles_immediately() {
    let (base_url, state) = platform_harness(CaptureAnswer::RefuseAuth).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::RefuseAuth, Arc::clone(&state)).await;
    fixture
        .deliver(message_update(9_501, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_501).await, "failed");
    assert_eq!(
        state.capture_calls.load(Ordering::SeqCst),
        1,
        "a permanent class must not retry"
    );
    assert_eq!(outbound_bodies(&fixture).await, [REFUSED_BODY]);
}

/// With the key tied to the body, a conflict can only mean "still in flight": it is retried
/// within the bound and exhaustion says the item was not saved yet, never that Platform refused
/// it (XR-021 CONTRACTS R4-08 rule 2).
#[tokio::test]
async fn a_conflict_is_retried_then_answered_unavailable_not_refused() {
    let (base_url, state) = platform_harness(CaptureAnswer::ConflictAlways).await;
    let fixture =
        Fixture::create(&base_url, CaptureAnswer::ConflictAlways, Arc::clone(&state)).await;
    fixture
        .deliver(message_update(9_531, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_531).await, "failed");
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        state.exchange_calls.load(Ordering::SeqCst),
        1,
        "a conflict is not a credential problem"
    );
    assert_eq!(outbound_bodies(&fixture).await, [UNAVAILABLE_BODY]);
}

/// A conflict that clears (the first request finished meanwhile) is accepted on the retry.
#[tokio::test]
async fn a_conflict_that_clears_on_the_second_attempt_is_accepted() {
    let (base_url, state) = platform_harness(CaptureAnswer::ConflictOnce).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::ConflictOnce, Arc::clone(&state)).await;
    fixture
        .deliver(message_update(9_541, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_541).await, "processed");
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 2);
    let operation: uuid::Uuid = OPERATION_ID.parse().expect("synthetic uuid");
    assert_eq!(binding_count(&fixture, operation).await, 1);
    assert_eq!(outbound_job_count(&fixture).await, 1);
    assert!(
        outbound_bodies(&fixture)
            .await
            .iter()
            .all(|body| body.starts_with("<b>Capturing")),
        "only the acknowledgment is enqueued"
    );
}

/// A reply that cannot be enqueued must not change the settled state: the update still fails.
#[tokio::test]
async fn a_reply_that_cannot_be_enqueued_still_settles_the_update_failed() {
    let (base_url, state) = platform_harness(CaptureAnswer::RefuseBadRequest).await;
    let fixture = Fixture::create(
        &base_url,
        CaptureAnswer::RefuseBadRequest,
        Arc::clone(&state),
    )
    .await;
    sqlx::query(
        "create function telegram.reject_failure_reply_for_test() returns trigger
         language plpgsql as $$
         begin
             raise exception 'synthetic reply insertion failure';
         end
         $$",
    )
    .execute(fixture.database.pool())
    .await
    .expect("the test trigger function");
    sqlx::query(
        "create trigger reject_failure_reply_for_test before insert on telegram.outbound_jobs
         for each row execute function telegram.reject_failure_reply_for_test()",
    )
    .execute(fixture.database.pool())
    .await
    .expect("the test trigger");

    fixture
        .deliver(message_update(9_431, "https://example.test/article"))
        .await;

    assert_eq!(fixture.settled_state(9_431).await, "failed");
    assert_eq!(outbound_job_count(&fixture).await, 0);
}
