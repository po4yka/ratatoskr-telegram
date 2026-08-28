## ADDED Requirements

### Requirement: Notification preferences are current-schema owned state

The current `telegram` schema SHALL store an explicit admitted private-chat-to-Telegram-identity binding and one notification policy per bound internal user and authorized Telegram chat, including a global enabled value, class-specific overrides, quiet-hours mode and bounds, high-priority bypass choice, optimistic version, and audit timestamps. The schema SHALL constrain policy tokens and quiet-hours invariants and SHALL reference only Telegram-owned identity/chat records.

#### Scenario: Fresh database receives the default policy

- **WHEN** an authorized private-chat binding first opens notification settings in a database created from the current `schema.sql`
- **THEN** it receives the documented enabled defaults without a migration ledger or copied domain state

#### Scenario: Invalid custom quiet hours are written directly

- **WHEN** a write attempts custom quiet hours with absent, equal, or out-of-range bounds
- **THEN** the database rejects the row and preserves the prior policy

#### Scenario: Preference targets an unbound chat

- **WHEN** a write attempts to create a notification policy for a chat not explicitly bound to the named Telegram identity
- **THEN** the database rejects the row and creates no inferred association

### Requirement: Notification decisions deduplicate transport and delivery

The current schema SHALL store the notification identity, recipient, target chat, class token, source event identity, decision state, defer-until time, outbound-job reference when one exists, and bounded failure class. Uniqueness SHALL prevent more than one decision for the same notification and target chat while allowing distinct eligible chats for one recipient.

#### Scenario: Same notification is admitted twice

- **WHEN** concurrent transactions admit two event envelopes carrying one notification for one target chat
- **THEN** one decision wins and at most one outbound job is linked

#### Scenario: One user configured two eligible chats

- **WHEN** one notification resolves to two independently authorized chats
- **THEN** each chat can hold one separate deduplicated decision
