## ADDED Requirements

### Requirement: The dispatcher connects under its own inbox prefix

The dispatcher's notification connection SHALL be opened with the custom inbox prefix `_INBOX_telegram`, held as a single constant, so that it needs to subscribe to nothing broader than `_INBOX_telegram.>`. The repository SHALL carry `deploy/nats/identity.conf`, a byte-equal copy (comments, whitespace and the nkey token ignored) of the `TELEGRAM` stanza of Platform's `ratatoskr.conf`.

#### Scenario: The consumer is opened under the Telegram stanza

- **WHEN** the dispatcher opens its pre-provisioned consumer against a broker whose only user is the Telegram stanza and a message is published to the stream
- **THEN** the consumer is returned and that message is fetched and acknowledged, because every JetStream reply arrives on `_INBOX_telegram`

#### Scenario: The prefix is missing

- **WHEN** a connection to that broker uses the default inbox prefix
- **THEN** the JetStream request is never answered to the client and times out
