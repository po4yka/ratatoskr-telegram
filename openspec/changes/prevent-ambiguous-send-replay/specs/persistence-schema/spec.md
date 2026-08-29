## ADDED Requirements

### Requirement: Outbound state records unknown provider outcomes

The current schema SHALL represent a terminal unknown-delivery outcome separately from retryable, sent, and permanently failed work. A known Telegram acknowledgement SHALL atomically settle its outbound job and persist every local binding, message-identity, callback, notification, and render effect required by that payload.

#### Scenario: Acknowledgement effects are all or nothing
- **WHEN** one local acknowledgement effect cannot be written
- **THEN** none of the acknowledgement effects or sent settlement commit and the transaction can be retried without a second wire request
