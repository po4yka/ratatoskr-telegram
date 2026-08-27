## MODIFIED Requirements

### Requirement: The terminal render carries truthful content and links

A successful terminal render SHALL compose the completion status, safe producer detail lines, and a Bot API deep-link button whose URL carries only an opaque expiring interaction token in the `start` query parameter; for a URL capture it SHALL additionally compose a fallback hyperlink to the captured article's address, and for an attachment capture it SHALL instead describe the received media by its type and size rather than fabricating an address. A failed terminal render SHALL compose the failure status with actionable guidance and safe error detail, and SHALL NOT offer a retry control or fabricate summary content that no upstream surface provided.

#### Scenario: Completion shows the link pair

- **WHEN** a followed operation reaches succeeded with no errors and its live interaction token exists
- **THEN** the terminal message contains the completion lead, an escaped hyperlink to the captured URL, and a button whose `https://t.me/<bot>?start=<token>` URL carries only the opaque token

#### Scenario: An attachment completion describes the media instead of inventing a link

- **WHEN** a followed attachment capture reaches succeeded and its interaction-token payload carries the blob reference and media facts
- **THEN** the terminal message names the received media type and size, contains no fabricated hyperlink, and still offers the opaque `start` button

#### Scenario: Failure tells the truth without a retry button

- **WHEN** a followed operation reaches failed carrying a typed extraction error
- **THEN** the terminal message shows the failed status and the escaped safe error line with guidance to resend the input, and contains no callback button and no invented summary

#### Scenario: A terminal render for an operation without an intent stays text-only

- **WHEN** a terminal render composes for a binding that has no live interaction-token record
- **THEN** the message keeps the status-led body and omits the deep-link button rather than failing the render

### Requirement: Deep-link intent records are opaque, expiring, and owner-bound

An operation intent created for a Telegram deep link SHALL use a high-entropy token from the shared interaction-token registry. The rendered Telegram URL SHALL put that token alone in the `start` query parameter, and the corresponding `/start <token>` payload SHALL be parsed only as an opaque token, never as business data. The server-side record SHALL bind the serving bot, owning Telegram user, originating chat, intent action, operation reference, bounded capture presentation payload, expiry, and one-time consumption state. Resolution SHALL require the complete scope and an unexpired unconsumed token; invalid, expired, replayed, malformed, or foreign presentations SHALL return no intent and SHALL not expose whether a scoped record exists.

#### Scenario: Valid start payload resolves once for its owner

- **WHEN** the owning user presents `/start <token>` under the bound bot and private chat before expiry
- **THEN** the parser releases the stored operation intent once and records its consumption without interpreting any token characters as an operation or address

#### Scenario: An expired intent stops resolving

- **WHEN** an intent is presented at or after its expiry, even by its owner
- **THEN** the lookup reports no intent and changes no operation state

#### Scenario: Another user cannot resolve a foreign intent

- **WHEN** a lookup for an existing unexpired intent names a different Telegram user
- **THEN** the lookup reports no intent and leaves the token available only to its bound owner

#### Scenario: Replayed start payload does not resolve twice

- **WHEN** the owner presents the same successfully consumed `/start` token again
- **THEN** the lookup reports no intent and releases no operation action a second time

#### Scenario: Business data is rejected as a start payload

- **WHEN** a `/start` payload carries a URL, JSON, provider identifier, or a value outside the opaque token grammar
- **THEN** it is rejected without looking up or executing business state
