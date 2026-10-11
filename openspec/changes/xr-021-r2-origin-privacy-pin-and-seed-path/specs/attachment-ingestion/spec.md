## ADDED Requirements

### Requirement: Only the forward member crosses the Platform boundary

A capture submission SHALL carry an `origin` member only when the capture has forward provenance, and that member SHALL contain the forward provenance and nothing else. The stored attachment facts (owner service, digest, media type, byte length) SHALL NOT appear in `origin`; they remain in the Telegram-owned intent record and cross the boundary only as the `blob` member of a blob capture.

#### Scenario: A forwarded PDF is submitted

- **WHEN** an authorized sender forwards a PDF within the limits and the capture is submitted
- **THEN** the request to Platform's blob route has an `origin` containing the forward fields and none of `owner_service`, `digest`, `media_type`, `length_bytes` or `blob`, its `blob` member is the stored BlobRef, and the intent record still stores the full metadata

#### Scenario: A PDF without a forward is submitted

- **WHEN** an authorized sender sends a PDF that is not a forward
- **THEN** the request to Platform carries no `origin` member
