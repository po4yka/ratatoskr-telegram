## ADDED Requirements

### Requirement: Dispatcher readiness includes notification consumption

When notification consumption is configured, dispatcher readiness SHALL reflect database availability, Bot API credential readability, NATS connectivity, event-stream/consumer compatibility, and whether the consumer loop is running. Readiness responses SHALL expose only stable dependency classes and SHALL NOT include addresses, subjects beyond the documented public contract, credentials, notification content, or user identifiers.

#### Scenario: Notification durable consumer cannot be opened

- **WHEN** the dispatcher cannot bind to the configured durable consumer
- **THEN** `/health/ready` reports not ready with a safe notification-bus dependency class and the process sends no notification

### Requirement: Operator health remains on the private operator plane

Service health SHALL remain available through the existing operator listener and metrics. Telegram user commands SHALL NOT expose process, database, NATS, credential, port, or queue diagnostics; `/status` SHALL retain its user operation-status meaning.

#### Scenario: User sends bare operator-style status request

- **WHEN** an authorized Telegram user invokes `/status` without an operation authority recognized by the existing interaction flow
- **THEN** the response does not reveal deployment health or internal topology
