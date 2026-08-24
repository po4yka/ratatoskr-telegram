## MODIFIED Requirements

### Requirement: Terminal settlement minimizes payload retention

The worker SHALL remove the processable Telegram payload in the same durable transition that records a terminal update state.

#### Scenario: processed payload is removed

- **WHEN** an update reaches processed, unsupported, failed, or denied state
- **THEN** its row retains only minimized deduplication and audit fields and no longer contains the processable payload
