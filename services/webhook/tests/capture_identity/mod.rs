//! The first session exchange binds the sender's Platform user id (XR-021 CONTRACTS S11).

use super::*;

/// The fake Platform mints `OPERATION_ID` as the user id of every exchange. After the sender's
/// first capture the identity row carries it, with no SQL seeding of `internal_user_id`.
#[tokio::test]
async fn a_first_capture_binds_the_senders_internal_user_id() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    let before: Option<uuid::Uuid> = sqlx::query_scalar(
        "select internal_user_id from telegram.identities where telegram_user_id = $1",
    )
    .bind(OWNER_TELEGRAM_USER_ID)
    .fetch_one(fixture.database.pool())
    .await
    .expect("the fixture owner identity");
    assert_eq!(before, None, "the fixture does not seed the binding");

    fixture
        .deliver(message_update(9_901, "https://example.test/article"))
        .await;
    assert_eq!(fixture.settled_state(9_901).await, "processed");

    let bound: Option<uuid::Uuid> = sqlx::query_scalar(
        "select internal_user_id from telegram.identities where telegram_user_id = $1",
    )
    .bind(OWNER_TELEGRAM_USER_ID)
    .fetch_one(fixture.database.pool())
    .await
    .expect("the owner identity");
    assert_eq!(
        bound,
        Some(OPERATION_ID.parse::<uuid::Uuid>().expect("synthetic uuid")),
        "the exchange must have bound the identity to the Platform user"
    );
}
