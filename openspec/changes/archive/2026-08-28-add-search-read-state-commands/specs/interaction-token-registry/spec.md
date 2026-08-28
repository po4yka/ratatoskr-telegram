## ADDED Requirements

### Requirement: Library read tokens use command-surface authority

The registry SHALL support a command-surface `library_read` action whose 64-character token references one accepted analysis identifier server-side and carries no target or content data client-side. The record SHALL bind bot, Telegram actor, internal user, and chat, SHALL expire 15 minutes after issue, and SHALL be single-use. Validation and consumption SHALL preserve the existing expiry, scope, concurrency, refusal, and cleanup guarantees; no dialogue transition or message binding is required for this action.

#### Scenario: One command presentation wins

- **WHEN** two workers concurrently present the same live `/read` token under its complete owner scope
- **THEN** exactly one receives the analysis action and the other receives a consumed refusal

#### Scenario: Forwarded command token cannot be used

- **WHEN** a live `/read` token is presented by another Telegram actor, internal user, bot, or chat
- **THEN** the registry releases no analysis identifier, changes no token state, and returns the same safe scoped refusal

#### Scenario: Cleanup removes an expired read token

- **WHEN** cleanup encounters an expired unconsumed library read token
- **THEN** it removes the token without changing Knowledge state, outbound jobs, or unrelated interaction authority
