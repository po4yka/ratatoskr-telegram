# attachment-ingestion Specification

## Purpose

Receiving document and photo attachments from an authorized private chat as first-class capture inputs: gated by type and size before any download, streamed through the Bot API inside a byte budget, hashed while streaming, stored in this service's own content-addressed blob store per the fleet `blob-references` convention, and handed to Platform as a capture that references stored bytes instead of a URL - with explicit truthful answers for attachment types this service does not ingest.

## Requirements

### Requirement: Attachments are gated on type and declared size before download

An authorized private message carrying a document SHALL be ingested only when its MIME type is `application/pdf`. Every other input - a photo, a document of any other MIME type or with none, video, voice, audio, animation, video note, or a sticker - SHALL produce exactly one static HTML reply stating that the type is not supported and that a PDF document with a text layer is what is supported, and SHALL settle the update without any Platform call. Photos are refused on purpose: image capture needs an OCR subsystem and is a separate follow-up. A PDF whose Bot API-declared byte size exceeds the configured ingestion limit SHALL receive one explicit reply naming the limit, and no file transfer SHALL start.

#### Scenario: A PDF document is accepted for ingestion

- **WHEN** an enabled sender in a private chat sends a document with MIME type `application/pdf` whose declared size is within the configured limit
- **THEN** the update proceeds to download and submission, not to an unsupported reply

#### Scenario: A photo is refused with the static reply

- **WHEN** an enabled sender sends a photo
- **THEN** exactly one outbound reply carries the static unsupported body, no Bot API file request, session exchange or capture call is made, and the update settles processed

#### Scenario: A non-PDF document is refused with the static reply

- **WHEN** an enabled sender sends a document whose MIME type is not `application/pdf`
- **THEN** exactly one outbound reply carries the static unsupported body and no Platform call is made

#### Scenario: A voice message gets an explicit truthful reply

- **WHEN** an enabled sender sends a voice message
- **THEN** exactly one outbound reply states the type is not supported, no session exchange or capture call reaches Platform, and the update settles

#### Scenario: An oversized document is refused before any transfer

- **WHEN** a PDF's declared `file_size` exceeds the configured ingestion limit
- **THEN** one reply names the limit and the update settles with no file download request issued to the Bot API harness

### Requirement: Attachment bytes are downloaded inside a bounded budget and hashed while streaming

The service SHALL resolve the file through the Bot API and stream its bytes to this service's blob store without holding the whole file in memory, aborting with a safe error class as soon as more than the configured byte budget arrives even when the declared size was smaller or absent. The SHA-256 digest SHALL be computed over the exact received bytes during the same pass, and the stored artifact's length SHALL equal the received byte count.

#### Scenario: A stream exceeding its budget aborts mid-download

- **WHEN** the harness serves more bytes than the configured budget for one requested file
- **THEN** the download aborts once the budget is exceeded, the update settles failed with a safe class, and no blob is published for the truncated bytes

#### Scenario: Received bytes hash to the published digest

- **WHEN** a document is downloaded and stored successfully
- **THEN** the blob reference published with the capture carries the SHA-256 hex of exactly the served bytes and their exact length

### Requirement: Stored attachments live in this service's own blob store under the fleet BlobRef convention

Stored attachment bytes SHALL be written to a telegram-owned content-addressed store - staged, then finalized under a path derived from the SHA-256 digest - and every consumer-facing reference SHALL be a BlobRef value carrying the owner service name of this repository, the `sha256` digest hex, a parameterless media type, and the byte length. Two stores of identical bytes SHALL yield the same BlobRef, and re-storing existing bytes SHALL NOT duplicate them. No host, path, credential, or expiry SHALL appear in any published BlobRef.

#### Scenario: Identical bytes converge on one blob reference

- **WHEN** the same document bytes are downloaded and stored twice
- **THEN** both stores publish the same owner service, digest, media type, and length

#### Scenario: A stored blob verifies against its own reference

- **WHEN** the store is asked to verify a previously stored attachment against its published BlobRef
- **THEN** verification passes on owner, algorithm, digest, media type, and length

### Requirement: A stored PDF submits as a capture on the blob route

A successfully stored PDF SHALL submit the same idempotent capture operation flow as a URL capture, except that the request goes to Platform's blob route `POST /v1/captures/blobs` with a body whose `blob` member is the stored BlobRef (owner service, sha256 digest, media type, byte length) instead of a URL; acknowledgment, binding, intent record and projection follow behave exactly as for URL captures. URL captures continue to use `POST /v1/captures`.

#### Scenario: A stored PDF becomes a blob-referencing capture

- **WHEN** an authorized sender sends a PDF within the limits and storage succeeds
- **THEN** Platform receives exactly one `POST /v1/captures/blobs` carrying the stored BlobRef and no URL, none is sent to `POST /v1/captures`, and the chat receives exactly one bound acknowledgment

#### Scenario: A URL capture keeps its route

- **WHEN** an authorized sender sends a bare URL
- **THEN** Platform receives the capture on `POST /v1/captures` with a `url` member

#### Scenario: Storage failure fails the update truthfully

- **WHEN** the blob store cannot accept the downloaded bytes, or the file cannot be resolved or downloaded, or its media type cannot be parsed
- **THEN** exactly one static reply says the attachment could not be read and asks the sender to resend it, the update settles failed, and no capture submission names a blob that was never durably stored

### Requirement: Every capture failure produces one static reply

When a capture submission ends in a transient class after its bounded retries, the sender SHALL receive one static reply that the service is temporarily unavailable and the item was not saved. When it ends in a permanent refusal, the sender SHALL receive one static reply that the item could not be saved. The reply SHALL be enqueued before the update settles `failed`; a failure to enqueue the reply SHALL NOT change that state. An accepted capture whose local projection is still pending SHALL keep its retry path and SHALL NOT produce a failure reply. No reply SHALL contain content derived from the failure, the request, or Platform's answer.

#### Scenario: Platform refuses the capture permanently

- **WHEN** Platform answers a capture with a permanent refusal
- **THEN** one static could-not-save reply is enqueued and the update settles failed after exactly one attempt

#### Scenario: Platform is unavailable

- **WHEN** Platform answers a capture with a transient failure on every bounded attempt
- **THEN** one static temporarily-unavailable reply is enqueued and the update settles failed

#### Scenario: The reply cannot be enqueued

- **WHEN** a capture fails and the reply insert also errors
- **THEN** the update still settles failed
