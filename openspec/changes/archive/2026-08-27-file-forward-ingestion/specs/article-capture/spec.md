## MODIFIED Requirements

### Requirement: Authorized private messages parse into capture intents

A processable message update from an authorized sender in a private chat SHALL be parsed into typed intents: a message whose text is a single http(s) URL SHALL become a capture intent, a `/summarize <url>` command with a well-formed URL argument SHALL become the same intent kind, and any other plain text - including a `/summarize` without a usable URL argument - SHALL leave the update settling as unsupported with class-only telemetry and no outbound traffic. A forwarded message whose text or caption contains an http(s) link SHALL become that same capture intent with its forward origin preserved as bounded metadata on the intent record and carried with the submission; when several links are present the first one is captured. A forwarded message carrying no link and no supported attachment settles unsupported.

#### Scenario: A bare URL becomes a capture intent

- **WHEN** an enabled sender in a private chat sends a message whose text is `https://example.test/article`
- **THEN** the update settles processed and one capture submission for that URL is attempted

#### Scenario: The summarize command form parses identically

- **WHEN** an enabled sender sends `/summarize https://example.test/article`
- **THEN** the update settles processed and the capture submission derives from the same intent kind and URL

#### Scenario: Text without a usable URL is unsupported

- **WHEN** an enabled sender sends `hello world`, or `/summarize` with no argument, or `/summarize ftp://example.test/x`
- **THEN** the update settles unsupported, nothing is submitted to Platform, and no message is sent

#### Scenario: A forwarded channel post with a link captures with provenance

- **WHEN** an enabled sender forwards a channel post whose text contains `https://example.test/story`
- **THEN** the capture submission for that URL proceeds and the intent record persists the forward origin facts (origin kind, origin identifiers, original date)

#### Scenario: The first of several forwarded links is captured

- **WHEN** a forwarded message's caption contains two http(s) links
- **THEN** exactly one capture submission results, referencing the first link in message order

#### Scenario: A forward with no link and no attachment is unsupported

- **WHEN** an enabled sender forwards a plain-text note containing no URL
- **THEN** the update settles unsupported with no submission and no outbound reply

### Requirement: Capture idempotency keys derive deterministically per sender, URL, and intent

The idempotency key submitted for a capture SHALL be derived deterministically from the sending Telegram user, the normalized source, and the intent kind, so resending the same link reuses the Platform operation. For URL captures normalization SHALL be limited to trimming surrounding whitespace and case-normalizing scheme and host; anything else distinguishes keys. For attachment captures the source SHALL be the stored blob's SHA-256 digest, so resending the identical file converges on one operation while a different file derives a different key. A deliberate retry after a FAILED operation SHALL salt the key with the failed operation identifier so it creates a new operation.

#### Scenario: Resending the same link reuses the operation

- **WHEN** the same sender submits `https://example.test/article` twice, the second after the first was accepted
- **THEN** both submissions carry the same idempotency key and Platform answers with the original operation identifier

#### Scenario: Host-case differences normalize to one key

- **WHEN** the same sender submits `HTTPS://Example.test/article` and later `https://example.test/article`
- **THEN** the derived key is identical for both

#### Scenario: The same file twice converges; a different file does not

- **WHEN** a sender uploads the same photo twice and also uploads a different photo
- **THEN** both uploads of the first photo derive one idempotency key from their shared digest, and the different photo derives another

#### Scenario: Retry after failure creates a new operation

- **WHEN** a capture for a URL reached the failed terminal state and the sender submits the same URL again through the retry flow
- **THEN** the derived key differs from the failed attempt's key because it names the failed operation, and a fresh operation results

### Requirement: The terminal render carries truthful content and links

A successful terminal render SHALL compose the completion status, safe producer detail lines, and a Mini App deep-link button whose parameter is an opaque expiring intent record owned by the requesting Telegram user and bound to the operation; for a URL capture it SHALL additionally compose a fallback hyperlink to the captured article's address, and for an attachment capture it SHALL instead describe the received media by its type and size rather than fabricating an address. A failed terminal render SHALL compose the failure status with actionable guidance and safe error detail, and SHALL NOT offer a retry control (callback tokens are a later plan item) nor fabricate summary content that no upstream surface provided.

#### Scenario: Completion shows the link pair

- **WHEN** a followed URL capture reaches succeeded with no errors and its intent record exists
- **THEN** the terminal message contains the completion lead, an escaped hyperlink to the captured URL, and a `startapp` button carrying only the opaque intent identifier

#### Scenario: An attachment completion describes the media instead of inventing a link

- **WHEN** a followed attachment capture reaches succeeded and its intent record carries the blob reference and media facts
- **THEN** the terminal message names the received media type and size, contains no fabricated hyperlink, and still offers the `startapp` button

#### Scenario: Failure tells the truth without a retry button

- **WHEN** a followed operation reaches failed carrying a typed extraction error
- **THEN** the terminal message shows the failed status and the escaped safe error line with guidance to resend the input, and contains no callback button and no invented summary

#### Scenario: A terminal render for an operation without an intent stays text-only

- **WHEN** a terminal render composes for a binding that has no intent record
- **THEN** the message keeps the status-led body and omits the deep-link button rather than failing the render
