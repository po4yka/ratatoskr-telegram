## ADDED Requirements

### Requirement: Callback confirmations persist as flow-bound one-time tokens

The current first-version schema SHALL persist a minimal callback flow record and its opaque tokens. The flow SHALL carry bot, owning Telegram user, originating chat, expected message, stable repository target, selected mode, current stage/version, expiry, and one stable action idempotency key; each app-minted token SHALL reference one flow transition, carry no business payload in its identifier, expire, and record one-time consumption. A transaction consuming a token SHALL lock and advance the expected flow version so concurrent confirm/cancel/replay attempts have one winner. No credential, raw provider response, message body, or free-form mutable policy JSON SHALL be stored.

#### Scenario: A fresh database enforces callback ownership and replay columns

- **WHEN** the root schema file is applied to an empty database
- **THEN** callback flow/token storage exists with opaque application-minted keys, owner/chat/message bindings, closed stage/action/state vocabularies, expiry, version, and consumption evidence

#### Scenario: Concurrent terminal decisions have one winner

- **WHEN** confirm and cancel tokens for the same expected flow version are consumed concurrently
- **THEN** one transition advances the flow and the other changes no state

#### Scenario: The schema stores references instead of secrets

- **WHEN** a repository confirmation flow is inspected
- **THEN** it contains the bounded stable target and action references needed for execution but no GitHub token, Platform session credential, raw callback payload, or private message body
