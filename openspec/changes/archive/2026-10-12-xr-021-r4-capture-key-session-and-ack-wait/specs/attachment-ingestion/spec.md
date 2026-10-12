## MODIFIED Requirements

### Requirement: A stored PDF submits as a capture on the blob route

A successfully stored PDF SHALL submit the same idempotent capture operation flow as a URL capture, except that the request goes to Platform's blob route `POST /v1/captures/blobs` with a body whose `blob` member is the stored BlobRef (owner service, sha256 digest, media type, byte length) instead of a URL; acknowledgment, binding, intent record and projection follow behave exactly as for URL captures. URL captures continue to use `POST /v1/captures`. The idempotency key SHALL describe the exact body, so the same PDF sent plain and then forwarded SHALL be two accepted operations and a media type that differs for the same bytes SHALL be a separate operation.

#### Scenario: A stored PDF becomes a blob-referencing capture

- **WHEN** an authorized sender sends a PDF within the limits and storage succeeds
- **THEN** Platform receives exactly one `POST /v1/captures/blobs` carrying the stored BlobRef and no URL, none is sent to `POST /v1/captures`, and the chat receives exactly one bound acknowledgment

#### Scenario: A URL capture keeps its route

- **WHEN** an authorized sender sends a bare URL
- **THEN** Platform receives the capture on `POST /v1/captures` with a `url` member

#### Scenario: A PDF sent plain and then forwarded yields two operations

- **WHEN** a sender sends a PDF and later forwards the same bytes with a forward origin
- **THEN** Platform accepts both submissions under different keys, both updates settle processed, two acknowledgments are enqueued and no refusal reply exists

#### Scenario: The same message sent twice converges

- **WHEN** the same PDF message is delivered twice
- **THEN** both submissions carry one key and Platform replays one operation

#### Scenario: Another media type is another operation

- **WHEN** the same bytes are submitted with a different media type
- **THEN** the derived key differs from the key of the first submission

#### Scenario: Storage failure fails the update truthfully

- **WHEN** the blob store cannot accept the downloaded bytes, or the file cannot be resolved or downloaded, or its media type cannot be parsed
- **THEN** exactly one static reply says the attachment could not be read and asks the sender to resend it, the update settles failed, and no capture submission names a blob that was never durably stored
