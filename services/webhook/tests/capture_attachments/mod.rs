//! Attachment intake: PDF documents become blob captures on the blob route; everything else gets
//! one static reply (XR-021 CONTRACTS S11).

use super::*;

/// An allowlisted PDF is fetched only through the synthetic Bot API, stored under its SHA-256,
/// then presented to Platform as the fleet `BlobRef` rather than a Telegram URL.
#[tokio::test]
async fn pdf_document_within_limits_stores_and_submits_a_blob_capture() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    fixture
        .deliver(pdf_document_update(9_801, "pdf-small", 128))
        .await;

    assert_eq!(
        fixture.settled_state(9_801).await,
        "processed",
        "get_file={} download={} platform={}",
        state.get_file_calls.load(Ordering::SeqCst),
        state.download_calls.load(Ordering::SeqCst),
        state.capture_calls.load(Ordering::SeqCst),
    );
    assert_eq!(
        state.capture_calls.load(Ordering::SeqCst),
        1,
        "an allowlisted PDF must reach the capture flow"
    );
    assert_eq!(
        *state.capture_paths.lock().expect("path lock"),
        ["/v1/captures/blobs"],
        "a blob capture goes to the blob route and never to the URL route"
    );
    assert_eq!(state.get_file_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.download_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        state
            .last_get_file_id
            .lock()
            .expect("file id lock")
            .as_deref(),
        Some("pdf-small")
    );

    let attachment = state
        .attachment_bytes
        .lock()
        .expect("attachment lock")
        .clone();
    let digest = format!("{:x}", Sha256::digest(&attachment));
    let body = state
        .last_capture_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("a capture body was posted");
    assert_eq!(
        body,
        json!({
            "blob": {
                "owner_service": "ratatoskr-telegram",
                "digest": {"algorithm": "sha256", "hex": digest},
                "media_type": "application/pdf",
                "length_bytes": attachment.len(),
            }
        }),
        "the Platform contract contains the exact fleet BlobRef and no URL: {body}"
    );
    assert!(
        fixture.has_published_blob(),
        "the completed blob was published"
    );

    let intent = sqlx::query(
        "select payload->>'source_url' as source_url, payload->'metadata' as metadata
         from telegram.interaction_tokens where operation_id = $1",
    )
    .bind(OPERATION_ID.parse::<uuid::Uuid>().expect("synthetic uuid"))
    .fetch_one(fixture.database.pool())
    .await
    .expect("an attachment intent row");
    let source_url: Option<String> = intent.get("source_url");
    let metadata: Value = intent.get("metadata");
    assert_eq!(source_url, None, "an attachment must not invent an address");
    assert_eq!(
        metadata,
        json!({
            "blob": {
                "owner_service": "ratatoskr-telegram",
                "algorithm": "sha256",
                "digest_hex": digest,
                "media_type": "application/pdf",
                "length_bytes": attachment.len(),
            }
        })
    );
    let body_text: String =
        sqlx::query_scalar("select payload->>'text' from telegram.outbound_jobs limit 1")
            .fetch_one(fixture.database.pool())
            .await
            .expect("attachment acknowledgment");
    assert!(body_text.contains("Capturing attachment"));
    assert!(!body_text.contains("<a href="), "no fabricated source link");
}

/// Photos are refusal-only (image capture needs an OCR subsystem): one static reply, no Bot API
/// file request, no session exchange, no Platform call.
#[tokio::test]
async fn a_photo_gets_an_unsupported_reply_and_no_platform_call() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    fixture.deliver(photo_update(9_811)).await;

    assert_eq!(fixture.settled_state(9_811).await, "processed");
    assert_eq!(state.get_file_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.download_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.exchange_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 0);
    assert_eq!(outbound_bodies(&fixture).await, [UNSUPPORTED_BODY]);
}

/// A document that is not a PDF is refused the same way, whatever its declared type is.
#[tokio::test]
async fn a_non_pdf_document_gets_an_unsupported_reply() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    fixture
        .deliver(document_update(9_812, "zip-file", 12, "application/zip"))
        .await;

    assert_eq!(fixture.settled_state(9_812).await, "processed");
    assert_eq!(state.get_file_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.exchange_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 0);
    assert_eq!(outbound_bodies(&fixture).await, [UNSUPPORTED_BODY]);
}

/// An oversized declared size is rejected before the service resolves or downloads a Bot API
/// file, and before it asks Platform for a session or operation.
#[tokio::test]
async fn oversized_declared_size_is_refused_before_any_download() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    fixture
        .deliver(pdf_document_update(9_821, "declared-oversize", 1_025))
        .await;

    assert_eq!(fixture.settled_state(9_821).await, "processed");
    assert_eq!(state.get_file_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.download_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.exchange_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 0);
    assert_eq!(outbound_job_count(&fixture).await, 1);
    let body: String = sqlx::query_scalar("select payload->>'text' from telegram.outbound_jobs")
        .fetch_one(fixture.database.pool())
        .await
        .expect("the safe limit reply");
    assert!(body.contains("Attachment too large"));
    assert!(body.contains("PDF document up to 1024 bytes"));
    assert!(
        !body.contains("photo"),
        "photos are no longer accepted: {body}"
    );
}

/// Declared metadata can be wrong. The streaming store is the final byte-budget authority: once
/// it observes an overrun, it publishes no content and the worker never submits a capture.
#[tokio::test]
async fn a_stream_overrunning_the_budget_fails_the_update_without_publishing_a_blob() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    *state.attachment_bytes.lock().expect("attachment lock") = vec![42; 1_025];
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    fixture
        .deliver(pdf_document_update(9_831, "stream-oversize", 1_024))
        .await;

    assert_eq!(fixture.settled_state(9_831).await, "failed");
    assert_eq!(state.get_file_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.download_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.exchange_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 0);
    assert!(
        !fixture.has_published_blob(),
        "partial bytes were never published"
    );
    assert_eq!(outbound_bodies(&fixture).await, [UNREADABLE_BODY]);
}

/// A Bot API download that fails after the file was resolved settles failed, but the sender is
/// told to send the attachment again and nothing reaches Platform.
#[tokio::test]
async fn an_attachment_download_failure_enqueues_a_could_not_read_reply() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    state.download_fails.store(true, Ordering::SeqCst);
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    fixture
        .deliver(pdf_document_update(9_832, "download-fails", 128))
        .await;

    assert_eq!(fixture.settled_state(9_832).await, "failed");
    assert_eq!(state.get_file_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.download_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 0);
    assert!(!fixture.has_published_blob());
    assert_eq!(outbound_bodies(&fixture).await, [UNREADABLE_BODY]);
}

/// Voice and video get one explicit reply each. None are downloaded, sent to
/// Platform, or misrepresented as a capture success; transcription belongs to another service.
#[tokio::test]
async fn unsupported_media_gets_one_explicit_truthful_reply() {
    let (base_url, state) = platform_harness(CaptureAnswer::Accept).await;
    let fixture = Fixture::create(&base_url, CaptureAnswer::Accept, Arc::clone(&state)).await;

    fixture
        .deliver(unsupported_media_update(9_841, "voice"))
        .await;
    fixture
        .deliver(unsupported_media_update(9_842, "video"))
        .await;

    for update_id in [9_841, 9_842] {
        assert_eq!(fixture.settled_state(update_id).await, "processed");
    }
    assert_eq!(state.get_file_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.download_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.exchange_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_calls.load(Ordering::SeqCst), 0);
    assert_eq!(outbound_job_count(&fixture).await, 2);
    let replies: Vec<String> =
        sqlx::query_scalar("select payload->>'text' from telegram.outbound_jobs order by id")
            .fetch_all(fixture.database.pool())
            .await
            .expect("truthful replies");
    assert_eq!(replies, [UNSUPPORTED_BODY, UNSUPPORTED_BODY]);
}
