## MODIFIED Requirements

### Requirement: Update deduplication state is persisted under bot identity

The schema SHALL contain a `telegram.updates` table whose primary key is `(bot_id, update_id)`, recording each admitted update's kind, its processing state drawn from `accepted`, `processing`, `processed`, `unsupported`, `failed` and `denied`, and its receipt and settle timestamps. No raw update payload SHALL be stored.

#### Scenario: A fresh database has the updates table

- **WHEN** the schema file is applied to an empty database
- **THEN** `telegram.updates` exists with the composite key and the state vocabulary including `denied`

#### Scenario: A second insert of the same pair changes nothing

- **WHEN** the same `(bot_id, update_id)` is recorded twice
- **THEN** the second attempt reports a duplicate and the table still holds one row

### Requirement: Processing state settles through typed transitions

An admitted row SHALL start as `accepted`; the processing worker SHALL move it through `processing` to exactly one terminal state — `processed`, `unsupported`, `failed` or `denied`. Recording a state for an update that was never admitted SHALL fail rather than write.

#### Scenario: An admitted update reaches a terminal state

- **WHEN** the worker settles an accepted row as processed
- **THEN** the row reads `processed` with a settle timestamp

#### Scenario: A state transition for an unknown update fails

- **WHEN** a settlement names a `(bot_id, update_id)` that was never inserted
- **THEN** the operation fails and no row appears

## ADDED Requirements

### Requirement: Identity bindings are persisted under Telegram identity

The schema SHALL contain a `telegram.identities` table whose primary key is `telegram_user_id`, recording for each known Telegram user an optional internal user reference, a display snapshot, an access state drawn from `enabled` and `disabled`, and creation and update timestamps. At most one row SHALL exist per Telegram user.

#### Scenario: A fresh database has the identities table

- **WHEN** the schema file is applied to an empty database
- **THEN** `telegram.identities` exists keyed by `telegram_user_id` with the access state vocabulary

#### Scenario: A second row for the same Telegram user is rejected

- **WHEN** a second identity row names a `telegram_user_id` that already exists
- **THEN** the insert is rejected and the table still holds one row for that user

### Requirement: Known chats are persisted under chat identity

The schema SHALL contain a `telegram.chats` table whose primary key is `chat_id`, recording for each known chat its type drawn from `private`, an access state drawn from `enabled` and `disabled`, and creation and update timestamps. At most one row SHALL exist per chat.

#### Scenario: A fresh database has the chats table

- **WHEN** the schema file is applied to an empty database
- **THEN** `telegram.chats` exists keyed by `chat_id` with the type and access state vocabularies

#### Scenario: A second row for the same chat is rejected

- **WHEN** a second chat row names a `chat_id` that already exists
- **THEN** the insert is rejected and the table still holds one row for that chat
