## MODIFIED Requirements

### Requirement: Every mode selection requires a second explicit confirmation

Each preview mode button SHALL carry only an opaque one-time selection token from the shared interaction-token registry and SHALL reference a durable repository dialogue at its expected version. Consuming a valid selection token SHALL advance the dialogue and render a confirmation prompt that names the exact mode and its effects and SHALL mint distinct opaque confirm and cancel tokens. Only consuming the valid confirm token SHALL submit the action; choosing cancel SHALL terminate the dialogue with no action. Telegram SHALL answer every recognized callback query promptly while domain work continues.

#### Scenario: Selecting star does not write

- **WHEN** the owner presses the preview's `star` selection button but has not pressed the confirmation button
- **THEN** Telegram answers the callback, advances the dialogue to confirmation, and renders the provider-write confirmation prompt while the GitHub harness records zero action calls

#### Scenario: Confirmed metadata action submits once

- **WHEN** the owner consumes the metadata selection token and then its valid confirm token
- **THEN** exactly one metadata action is submitted with that dialogue's stable idempotency key and confirmation evidence reference

#### Scenario: Cancellation performs no action

- **WHEN** the owner consumes the cancel token from a confirmation prompt
- **THEN** the dialogue becomes cancelled, the callback is answered, and no action request is submitted

### Requirement: Callback tokens are opaque, expiring, owner-bound, and replay-safe

Repository confirmation state SHALL bind bot, Telegram user, chat, expected message, stable preview target, mode, stage, expiry, and idempotency key behind the shared registry's app-minted high-entropy tokens. Token consumption SHALL be transactional and require the expected dialogue stage/version and owner/chat/message binding; expired, replayed, malformed, stale-stage, or foreign tokens SHALL submit nothing. A repeated or stale press SHALL be answered with the common expired-state guidance rather than re-executing or exposing why the authority is unavailable. Callback data SHALL contain no raw URL, repository JSON, account identity, provider credential, or mutable policy state.

#### Scenario: A foreign forwarded button cannot act

- **WHEN** another Telegram user presents an unexpired confirmation token created for the owner
- **THEN** the callback is answered with the common safe refusal and GitHub receives no action request

#### Scenario: Concurrent replay has one winner

- **WHEN** two workers attempt to consume the same confirmation token concurrently
- **THEN** exactly one dialogue transition submits the action and the other is answered with expired-state guidance without another action call

#### Scenario: Expired confirmation cannot write

- **WHEN** the owner presses a confirmation token at or after its expiry
- **THEN** Telegram answers it with expired-state guidance and submits no action
