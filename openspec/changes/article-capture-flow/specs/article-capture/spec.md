## Purpose

Turning an authorized private-message URL or `/summarize` command into an idempotent Platform capture operation, acknowledging it in one bound chat message, following the operation to a truthful terminal render with links - the first product slice of the Telegram integration.

## ADDED Requirements

### Requirement: Authorized private messages parse into capture intents

A processable message update from an authorized sender in a private chat SHALL be parsed into typed intents: a message whose text is a single http(s) URL SHALL become a capture intent, a `/summarize <url>` command with a well-formed URL argument SHALL become the same intent kind, and any other text - including a `/summarize` without a usable URL argument - SHALL leave the update settling as unsupported with class-only telemetry and no outbound traffic.

#### Scenario: A bare URL becomes a capture intent

- **WHEN** an enabled sender in a private chat sends a message whose text is `https://example.test/article`
- **THEN** the update settles processed and one capture submission for that URL is attempted

#### Scenario: The summarize command form parses identically

- **WHEN** an enabled sender sends `/summarize https://example.test/article`
- **THEN** the update settles processed and the capture submission derives from the same intent kind and URL

#### Scenario: Text without a usable URL is unsupported

- **WHEN** an enabled sender sends `hello world`, or `/summarize` with no argument, or `/summarize ftp://example.test/x`
- **THEN** the update settles unsupported, nothing is submitted to Platform, and no message is sent

### Requirement: Capture idempotency keys derive deterministically per sender, URL, and intent

The idempotency key submitted for a capture SHALL be derived deterministically from the sending Telegram user, the normalized URL, and the intent kind, so resending the same link reuses the Platform operation. Normalization SHALL be limited to trimming surrounding whitespace and case-normalizing scheme and host; anything else distinguishes keys. A deliberate retry after a FAILED operation SHALL salt the key with the failed operation identifier so it creates a new operation.

#### Scenario: Resending the same link reuses the operation

- **WHEN** the same sender submits `https://example.test/article` twice, the second after the first was accepted
- **THEN** both submissions carry the same idempotency key and Platform answers with the original operation identifier

#### Scenario: Host-case differences normalize to one key

- **WHEN** the same sender submits `HTTPS://Example.test/article` and later `https://example.test/article`
- **THEN** the derived key is identical for both

#### Scenario: Retry after failure creates a new operation

- **WHEN** a capture for a URL reached the failed terminal state and the sender submits the same URL again through the retry flow
- **THEN** the derived key differs from the failed attempt's key because it names the failed operation, and a fresh operation results

### Requirement: Submission authenticates through a short-lived assertion exchange

The service SHALL authenticate capture submissions and operation reads to Platform by exchanging a short-lived Ed25519-signed identity assertion - issuer `ratatoskr-telegram`, subject the Telegram user identifier, audience the configured Platform audience, single-use nonce - on Platform's exchange route, and SHALL present the returned bearer credential. Sessions SHALL be cached per sender until shortly before expiry and re-exchanged thereafter. The assertion signing key SHALL exist only as configuration secret in this service.

#### Scenario: A cached session is reused across captures

- **WHEN** the same sender submits two captures within the session lifetime against a recording Platform harness
- **THEN** the exchange route is called once and both submissions carry that session's credential

#### Scenario: Session expiry forces a re-exchange

- **WHEN** a capture is attempted after the cached session's near-expiry boundary
- **THEN** a fresh assertion is exchanged before the submission proceeds

### Requirement: An accepted capture acknowledges with one bound message

After Platform accepts a capture, the service SHALL pre-create the operation's message binding for the requesting chat and enqueue exactly one acknowledgment send job referencing the operation, so progress arriving before the send completes lands on the eventual message instead of being dropped as unbound. A repeated submission that resolves to an operation already bound live in the same chat SHALL NOT enqueue a second acknowledgment.

#### Scenario: Early progress is not lost to an unsent ack

- **WHEN** Platform's first progress frame for an operation arrives while its acknowledgment send is still queued
- **THEN** the frame is applied to the binding and renders once the message exists, rather than counting as unbound traffic

#### Scenario: Double send produces one tracked message

- **WHEN** the same sender submits the same URL twice and both resolve to one operation
- **THEN** the chat holds exactly one acknowledged message bound to that operation

### Requirement: Live operations are followed over Platform's event stream

The dispatcher SHALL follow every non-terminal bound operation by consuming Platform's per-operation SSE event stream, mapping each frame onto the internal projection seam with the frame identifier as the deduplicating event id and the frame timestamp as occurrence time, resuming with the last seen event identifier after a reconnect, and stopping when the stream reports a terminal status. Following state across restarts SHALL be recovered from the bindings themselves, so each non-terminal operation is followed exactly once after a restart.

#### Scenario: Frames become throttled projections

- **WHEN** an followed operation's stream delivers accepted then running frames
- **THEN** the bound message is edited according to the existing projection guards and throttle arithmetic

#### Scenario: A redelivered frame changes nothing twice

- **WHEN** a reconnect replays a frame whose identifier was already consumed
- **THEN** the replayed frame is dropped by event deduplication and no extra edit is enqueued

#### Scenario: A restarted dispatcher follows each live operation once

- **WHEN** the dispatcher restarts while three operations sit non-terminal and one terminal in its bindings
- **THEN** it opens streams for exactly the three non-terminal operations and none for the terminal one

### Requirement: The terminal render carries truthful content and links

A successful terminal render SHALL compose the completion status, safe producer detail lines, a fallback hyperlink to the captured article's address, and a Mini App deep-link button whose parameter is an opaque expiring intent record owned by the requesting Telegram user and bound to the operation. A failed terminal render SHALL compose the failure status with actionable guidance and safe error detail, and SHALL NOT offer a retry control (callback tokens are a later plan item) nor fabricate summary content that no upstream surface provided.

#### Scenario: Completion shows the link pair

- **WHEN** a followed operation reaches succeeded with no errors and its intent record exists
- **THEN** the terminal message contains the completion lead, an escaped hyperlink to the captured URL, and a `startapp` button carrying only the opaque intent identifier

#### Scenario: Failure tells the truth without a retry button

- **WHEN** a followed operation reaches failed carrying a typed extraction error
- **THEN** the terminal message shows the failed status and the escaped safe error line with guidance to resend the link, and contains no callback button and no invented summary

#### Scenario: A terminal render for an operation without an intent stays text-only

- **WHEN** a terminal render composes for a binding that has no intent record
- **THEN** the message keeps the status-led body and omits the deep-link button rather than failing the render

### Requirement: Deep-link intent records are opaque, expiring, and owner-bound

An intent record created for a deep link SHALL carry an app-minted high-entropy identifier that appears alone in the deep-link parameter, the owning bot and Telegram user, the originating chat, an intent-kind vocabulary value, the operation reference and source URL it presents, and an expiry; lookups SHALL match only unexpired rows and only for the owning user. The record SHALL contain no credentials and no content beyond the submitted URL reference.

#### Scenario: An expired intent stops resolving

- **WHEN** an intent past its expiry is looked up, even by its owner
- **THEN** the lookup reports no intent

#### Scenario: Another user cannot resolve a foreign intent

- **WHEN** a lookup for an existing unexpired intent names a different Telegram user
- **THEN** the lookup reports no intent

### Requirement: Platform failures degrade honestly and boundedly

When Platform cannot accept a capture - unreachable, timing out, refusing authentication, or rejecting the request - the worker SHALL retry transient classes within a small bounded attempt count and then settle the update as failed with class-only telemetry; permanent client-class refusals SHALL settle immediately without retries. No acknowledgment message SHALL be sent for a capture that was never accepted.

#### Scenario: An unreachable Platform fails the update without an ack

- **WHEN** every submit attempt inside the attempt bound fails with network errors
- **THEN** the update settles failed, the failure class is counted, and no outbound message exists for it

#### Scenario: A refused submission settles immediately

- **WHEN** Platform rejects the submission with a permanent client-error class
- **THEN** the update settles failed on that first answer with no further attempts
