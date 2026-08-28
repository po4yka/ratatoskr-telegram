## ADDED Requirements

### Requirement: Notification outcomes use bounded content-free telemetry

Telemetry SHALL count notification receipt, duplicate, enabled, suppressed, deferred, enqueued, delivered, retry, and terminal failure outcomes using closed safe labels. It SHALL expose consumer lag and decision backlog without usernames, Telegram or internal user/chat identifiers, class tokens from unknown producers, titles, messages, URLs, correlation references, credentials, or raw errors as metric labels.

#### Scenario: Unknown notification class is processed

- **WHEN** a well-formed class unknown to this build is admitted
- **THEN** telemetry uses the bounded class label `other` and does not copy the producer token into labels or logs

#### Scenario: Preference suppresses a notification

- **WHEN** a class toggle prevents delivery
- **THEN** the suppression counter advances using a safe reason and no content-bearing field is emitted
