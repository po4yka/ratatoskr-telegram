## MODIFIED Requirements

### Requirement: Capture idempotency keys derive deterministically per sender, URL, and intent

The idempotency key submitted for a capture SHALL be `hex(sha256("capture.v2|" || sender || "|" || route path || "|" || request body bytes))`, where the route path and the body are produced by the same function the Platform client uses to send the request, so a key never describes a body other than the one sent. The same body therefore replays its original operation, and a different body (a forwarded copy, another media type) is a separate operation. For URL captures normalization SHALL be limited to trimming surrounding whitespace and case-normalizing scheme and host; anything else distinguishes keys. For attachment captures the body carries the stored blob's SHA-256 digest, so resending the identical file converges on one operation while a different file derives a different key. A deliberate retry after a FAILED operation SHALL salt the key with the failed operation identifier so it creates a new operation.

#### Scenario: Resending the same link reuses the operation

- **WHEN** the same sender submits `https://example.test/article` twice, the second after the first was accepted
- **THEN** both submissions carry the same idempotency key and Platform answers with the original operation identifier

#### Scenario: Host-case differences normalize to one key

- **WHEN** the same sender submits `HTTPS://Example.test/article` and later `https://example.test/article`
- **THEN** the derived key is identical for both

#### Scenario: A bare link and the same link forwarded are two operations

- **WHEN** a sender submits `https://example.test/story` bare and then forwards a message whose first link is the same address
- **THEN** the two submissions carry different keys, Platform accepts both, no refusal reply is enqueued, and each acceptance binds its own acknowledgment

#### Scenario: The same file twice converges; a different file does not

- **WHEN** a sender uploads the same photo twice and also uploads a different photo
- **THEN** both uploads of the first photo derive one idempotency key from their shared body, and the different photo derives another

#### Scenario: Two different bodies never share a key

- **WHEN** two submissions differ in sender, route, any body member or the failed-operation salt
- **THEN** their derived keys differ, and two submissions with the same sender, route and body derive the same key

#### Scenario: Retry after failure creates a new operation

- **WHEN** a capture for a URL reached the failed terminal state and the sender submits the same URL again through the retry flow
- **THEN** the derived key differs from the failed attempt's key because it names the failed operation, and a fresh operation results

### Requirement: Platform failures degrade honestly and boundedly

When Platform cannot accept a capture - unreachable, timing out, answering a conflict, refusing authentication, or rejecting the request - the worker SHALL retry transient classes (network, timeout, server error, rate limit and conflict) within a small bounded attempt count and then settle the update as failed with class-only telemetry; permanent client-class refusals SHALL settle immediately without retries. When Platform rejects the session credential the worker SHALL drop that credential from the cache only if it is still the current entry, exchange a fresh session and submit once more; a second credential rejection SHALL be a permanent refusal. No acknowledgment message SHALL be sent for a capture that was never accepted.

#### Scenario: An unreachable Platform fails the update without an ack

- **WHEN** every submit attempt inside the attempt bound fails with network errors
- **THEN** the update settles failed, the failure class is counted, and no outbound message exists for it

#### Scenario: A refused submission settles immediately

- **WHEN** Platform rejects the submission with a permanent client-error class
- **THEN** the update settles failed on that first answer with no further attempts

#### Scenario: A conflict is retried and then reported as unavailable

- **WHEN** Platform answers a capture with a conflict on every bounded attempt
- **THEN** the capture is attempted twice, the sender receives the temporarily-unavailable reply and not the could-not-save reply, and the update settles failed

#### Scenario: A conflict that clears is accepted

- **WHEN** Platform answers the first attempt with a conflict and the second with an acceptance
- **THEN** the capture is accepted and acknowledged once

#### Scenario: A rejected session is exchanged again

- **WHEN** Platform rejects the first bearer credential with 401 and accepts the credential of a second exchange
- **THEN** the session route is called twice, the capture is accepted, and this holds for URL and blob captures

#### Scenario: A second credential rejection is permanent

- **WHEN** Platform rejects the credential and also the freshly exchanged one
- **THEN** exactly one further exchange and one further attempt are made and the update settles failed with the could-not-save reply

#### Scenario: Invalidation never removes a newer credential

- **WHEN** a rejected credential is invalidated after a newer one has replaced it in the cache
- **THEN** the newer credential stays cached
