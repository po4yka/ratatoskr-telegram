## ADDED Requirements

### Requirement: Known external acceptance remains recoverable until local handoff commits

When processing establishes that an idempotent external command was accepted but its required local handoff cannot commit, the worker SHALL retain the durable source payload in a processable state and retry with the same external command identity. It SHALL terminally minimize the payload only after the local handoff has committed or an explicit external refusal makes recovery unnecessary.

#### Scenario: Worker restarts after accepted capture
- **WHEN** Platform accepted a capture and the webhook stops before its local projection commits
- **THEN** a new worker reclaims the retained update and converges on the same operation without creating partial or duplicate local projections
