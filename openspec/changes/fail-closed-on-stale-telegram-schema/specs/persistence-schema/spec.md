## MODIFIED Requirements

### Requirement: Schema application is startup work, idempotent and all-or-nothing

When a database is configured, a process SHALL establish before reporting ready that the database exactly matches the one current schema definition embedded in the running binary. Fresh application SHALL be safe to run concurrently and SHALL either apply completely, including durable match evidence, or leave the database unchanged. An existing matching schema SHALL make no changes; an existing schema with absent or different match evidence SHALL fail startup without altering it.

#### Scenario: Applying twice changes nothing the second time
- **WHEN** the same embedded schema is applied to a database that already records an exact match
- **THEN** the second application makes no changes and reports success

#### Scenario: An older development schema is refused
- **WHEN** startup finds the Telegram namespace without match evidence for the running binary's embedded schema
- **THEN** startup reports a safe stale-schema failure and makes no database changes

#### Scenario: A different current definition is refused
- **WHEN** startup finds match evidence for schema contents different from the running binary's embedded definition
- **THEN** startup reports a safe schema-mismatch failure and makes no database changes
