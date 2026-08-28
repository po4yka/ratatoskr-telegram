## ADDED Requirements

### Requirement: Notification consumption uses typed finite configuration

The dispatcher configuration SHALL define the NATS endpoint, event stream, durable consumer, exact notification subject, fetch/ack limits, and credential-file path under the existing Ratatoskr environment prefix. Unknown fields, non-TLS remote endpoints outside the local deployment allowance, wildcard notification subjects, zero bounds, and conflicting inline/file credentials SHALL be refused before listeners bind.

#### Scenario: Dispatcher is configured for the canonical subject

- **WHEN** configuration names `ratatoskr_events`, durable `ratatoskr_telegram_notifications`, and `evt.platform.notification.raised.v1` with positive bounds and a readable credential file
- **THEN** validation accepts the notification-consumer section without exposing credential contents

#### Scenario: Subject is widened to all events

- **WHEN** the notification subject is configured as `evt.>`
- **THEN** validation refuses startup because Telegram requires only the canonical notification fact

### Requirement: Runtime secrets support atomic file rotation

Bot token, webhook secret, NATS credential, and Platform signing material SHALL be loadable from explicit credential files with one configured source per secret. Effective configuration and validation errors SHALL render only source metadata and safe failure classes, never file contents.

#### Scenario: Inline and file secret are both configured

- **WHEN** a secret has both an inline value and a credential-file path
- **THEN** configuration fails closed and names the conflicting settings without rendering either value
