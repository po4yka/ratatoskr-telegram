## MODIFIED Requirements

### Requirement: Notification consumption uses typed finite configuration

The dispatcher configuration SHALL define the NATS endpoint, event stream, durable consumer, exact notification subject, fetch/ack limits, and credential-file path under the existing Ratatoskr environment prefix. The ack wait SHALL equal `NOTIFICATION_ACK_WAIT_SECONDS` (30), the value at which Edge provisions the `ratatoskr_telegram_notifications` durable; the key exists only to state that value and any other value SHALL stop startup with a violation that names Edge's fixed durable. Unknown fields, non-TLS remote endpoints outside the local deployment allowance, wildcard notification subjects, zero bounds, and conflicting inline/file credentials SHALL be refused before listeners bind.

#### Scenario: Dispatcher is configured for the canonical subject

- **WHEN** configuration names `ratatoskr_events`, durable `ratatoskr_telegram_notifications`, and `evt.platform.notification.raised.v1` with positive bounds and a readable credential file
- **THEN** validation accepts the notification-consumer section without exposing credential contents

#### Scenario: Subject is widened to all events

- **WHEN** the notification subject is configured as `evt.>`
- **THEN** validation refuses startup because Telegram requires only the canonical notification fact

#### Scenario: An ack wait other than 30 is refused

- **WHEN** the ack wait is configured as 29 or 31
- **THEN** validation reports a violation whose message names Edge's fixed durable, while 30 and the default are accepted
