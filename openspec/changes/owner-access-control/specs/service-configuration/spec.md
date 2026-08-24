## ADDED Requirements

### Requirement: Access configuration carries the bootstrap owner

Configuration SHALL read the owner Telegram user id from `RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID` as a positive signed 64-bit integer. For the webhook role the value SHALL be required like other intake requirements; the dispatcher role SHALL NOT require it.

#### Scenario: A valid Telegram user id passes validation

- **WHEN** `RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID` is a positive integer within the signed 64-bit range
- **THEN** configuration validates and no other setting changes

#### Scenario: A non-positive or non-integer value is refused

- **WHEN** the owner Telegram user id is zero, negative, or not an integer
- **THEN** the process exits 78 and the violation names the key without echoing the value

#### Scenario: The webhook refuses to validate without the owner

- **WHEN** the webhook binary runs with defaults and no owner Telegram user id configured
- **THEN** validation fails and the report names the missing key alongside the other intake requirements

#### Scenario: The dispatcher still starts without the owner

- **WHEN** the dispatcher binary runs with only default configuration
- **THEN** it starts and serves its operator plane
