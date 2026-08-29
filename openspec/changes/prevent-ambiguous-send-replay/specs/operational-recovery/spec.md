## ADDED Requirements

### Requirement: Unknown send outcomes require explicit recovery

Recovery tooling SHALL expose non-idempotent send jobs whose provider outcome is unknown as quarantined records with bounded identifiers and safe failure classes. It SHALL NOT make them generally claimable. Any later resend SHALL require an explicit operator-authorized action that rechecks the quarantine state and warns that Telegram may already have delivered the original message.

#### Scenario: Operator inspects an ambiguous send
- **WHEN** an operator inspects a quarantined send after process loss
- **THEN** the record reports delivery as unknown without message content and remains ineligible for automatic replay
