## ADDED Requirements

### Requirement: Non-idempotent sends preserve honest delivery uncertainty

After Telegram returns a successful `sendMessage` response, the dispatcher SHALL persist the job settlement, returned provider message identity, binding effects, and render advancement atomically and SHALL retry only that idempotent local recording while the acknowledgement remains known. A send whose provider outcome is ambiguous or whose lease expires without a persisted acknowledgement SHALL enter an explicit unknown-outcome quarantine and SHALL NOT be sent automatically again. Automatic send retry SHALL require provider evidence that the request was not applied. Idempotent edits MAY retain their bounded stale-lease retry behavior.

#### Scenario: Known acknowledgement survives a local commit failure
- **WHEN** Telegram acknowledges a send and the first local acknowledgement transaction fails
- **THEN** the dispatcher retries the local transaction without another Bot API send and eventually records all acknowledgement effects together

#### Scenario: Stale send has unknown outcome
- **WHEN** a `sendMessage` lease expires without a durable provider acknowledgement
- **THEN** the job is quarantined as outcome unknown and no automatic claim performs another wire request

#### Scenario: Explicit Telegram refusal is retryable when eligible
- **WHEN** Telegram definitively rejects a send with a retryable not-applied response
- **THEN** the job follows bounded retry policy without being classified as an ambiguous delivery
