## Purpose

Provides one durable authority for opaque Telegram callback and deep-link tokens so actions remain scoped, expiring, replay-safe, and free of client-carried business state.

## ADDED Requirements

### Requirement: Tokens are opaque high-entropy references to server-side actions

The service SHALL mint URL-safe tokens from at least 256 bits of operating-system randomness. A presented token SHALL identify a server-side record containing its surface, typed action and bounded payload, bot, Telegram user, chat, optional expected message and dialogue version, creation time, expiry, and one-time consumption state. Callback data and deep-link parameters SHALL carry only the token and SHALL contain no business payload, resource address, provider identity, credential, or mutable policy.

#### Scenario: Issued callback and deep-link values disclose no action data

- **WHEN** callback and deep-link tokens are issued for a stored action containing repository and operation references
- **THEN** each client-visible value is URL-safe, fits the Telegram surface limit, and contains none of those stored references

### Requirement: Token consumption enforces expiry and complete scope

The service SHALL resolve and consume a token only when its surface, bot, Telegram user, chat, optional message binding, and optional expected dialogue version all match and its expiry is strictly later than the presentation time. Invalid, expired, malformed, stale-version, or scope-mismatched presentations SHALL return a typed refusal and SHALL change no dialogue, token, operation, or outbound action state.

#### Scenario: Token expires at its boundary

- **WHEN** the owner presents a token at or after its expiry
- **THEN** consumption returns expired and the stored action is not released

#### Scenario: Foreign scope cannot consume a live token

- **WHEN** a live token is presented under a different bot, Telegram user, chat, or expected message
- **THEN** consumption returns a safe scope refusal and neither the token nor its referenced state changes

### Requirement: One-time consumption has one transactional winner

A single-use token SHALL be consumed in the same transaction that validates its scope and advances its referenced dialogue state. Concurrent or later presentations SHALL never release the action again. A recognized callback that loses to expiry, prior consumption, or stale state SHALL still be answered promptly with the stable expired-state message `This action has expired. Please start again.` and SHALL perform no action.

#### Scenario: Concurrent presentations execute once

- **WHEN** two workers concurrently consume the same valid single-use token
- **THEN** exactly one receives the stored action and commits its transition while the other receives a consumed refusal

#### Scenario: Second callback press receives expired-state guidance

- **WHEN** the owner presses an already consumed callback token a second time
- **THEN** Telegram answers the callback with `This action has expired. Please start again.` and does not execute or enqueue the stored action again

### Requirement: Cleanup bounds stale token retention

The webhook runtime SHALL run an idempotent bounded cleanup pass at startup and on a fixed interval. Each pass SHALL remove only expired or consumed interaction-token records that are no longer needed by a live dialogue or a non-terminal operation follower; a missing cleaned token SHALL resolve through the same safe expired-state path and cleanup SHALL not delete domain operations, message bindings, or unrelated service state.

#### Scenario: Cleanup removes stale tokens without affecting live authority

- **WHEN** one cleanup batch sees expired, consumed, and live tokens
- **THEN** it removes only eligible stale rows, leaves live authority and ownership records needed by non-terminal operation followers resolvable, and changes no operation or message binding
