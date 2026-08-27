# attachment-ingestion Specification

## Purpose

Receiving document and photo attachments from an authorized private chat as first-class capture inputs: gated by type and size before any download, streamed through the Bot API inside a byte budget, hashed while streaming, stored in this service's own content-addressed blob store per the fleet `blob-references` convention, and handed to Platform as a capture that references stored bytes instead of a URL - with explicit truthful answers for attachment types this service does not ingest.

## Requirements

### Requirement: Attachments are gated on type and declared size before download

An authorized private message carrying a document SHALL be ingested only when its MIME type is in the supported allowlist (`application/pdf`) or it is a photo (largest available size, `image/jpeg`); any other media kind - video, voice, audio, animation, video note, sticker, or a document of an unlisted MIME type - SHALL produce exactly one explicit HTML reply stating that the input type is not supported yet and what is supported, and SHALL settle the update without any Platform call. A document or photo whose Bot API-declared byte size exceeds the configured ingestion limit SHALL receive one explicit reply naming the limit, and no file transfer SHALL start.

#### Scenario: A PDF document is accepted for ingestion

- **WHEN** an enabled sender in a private chat sends a document with MIME type `application/pdf` whose declared size is within the configured limit
- **THEN** the update proceeds to download and submission, not to an unsupported reply

#### Scenario: A voice message gets an explicit truthful reply

- **WHEN** an enabled sender sends a voice message
- **THEN** exactly one outbound reply states the type is not supported yet, no session exchange or capture call reaches Platform, and the update settles

#### Scenario: An oversized document is refused before any transfer

- **WHEN** a document's declared `file_size` exceeds the configured ingestion limit
- **THEN** one reply names the refusal class and the update settles with no file download request issued to the Bot API harness

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

### Requirement: A stored attachment submits as a capture referencing the blob

A successfully stored attachment SHALL submit the same idempotent capture operation flow as a URL capture, except the submitted payload references the stored BlobRef instead of a URL and carries the attachment's media type; acknowledgment, binding, intent record, projection follow, and failure degradation all behave exactly as for URL captures.

#### Scenario: A stored photo becomes a blob-referencing capture

- **WHEN** an authorized sender sends a photo within the limits and storage succeeds
- **THEN** one capture submission carries the stored BlobRef (owner service of this deployment, sha256 digest, image media type, byte length) and no fabricated URL, and the chat receives exactly one bound acknowledgment

#### Scenario: Storage failure fails the update truthfully

- **WHEN** the blob store cannot accept the downloaded bytes within the attempt bounds
- **THEN** the update settles failed with a safe storage class and no capture submission names a blob that was never durably stored
