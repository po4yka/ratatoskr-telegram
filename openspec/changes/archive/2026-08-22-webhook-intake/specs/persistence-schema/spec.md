## Purpose

The `telegram` PostgreSQL schema this service owns, how it is applied at startup, and the readiness check that watches it.

## ADDED Requirements

### Requirement: Update deduplication state is persisted under bot identity

The schema SHALL contain a `telegram.updates` table whose primary key is `(bot_id, update_id)`, recording each admitted update's kind, its processing state drawn from `accepted`, `processing`, `processed`, `unsupported` and `failed`, and its receipt and settle timestamps. No raw update payload SHALL be stored.

#### Scenario: A fresh database has the updates table

- **WHEN** the schema file is applied to an empty database
- **THEN** `telegram.updates` exists with the composite key and the state vocabulary

#### Scenario: A second insert of the same pair changes nothing

- **WHEN** the same `(bot_id, update_id)` is recorded twice
- **THEN** the second attempt reports a duplicate and the table still holds one row

### Requirement: Processing state settles through typed transitions

An admitted row SHALL start as `accepted`; the processing worker SHALL move it through `processing` to exactly one terminal state — `processed`, `unsupported` or `failed`. Recording a state for an update that was never admitted SHALL fail rather than write.

#### Scenario: An admitted update reaches a terminal state

- **WHEN** the worker settles an accepted row as processed
- **THEN** the row reads `processed` with a settle timestamp

#### Scenario: A state transition for an unknown update fails

- **WHEN** a settlement names a `(bot_id, update_id)` that was never inserted
- **THEN** the operation fails and no row appears
