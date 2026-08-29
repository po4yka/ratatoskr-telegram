## ADDED Requirements

### Requirement: Submitting dialogue recovery is bound to its releasing update

A dialogue that releases external work SHALL durably record the admitted update that won the transition into `submitting`. Only reprocessing that same durable update SHALL resume the transition; foreign, duplicated under another update identity, and stale inputs SHALL change no state and release no work. Completion and its required outbound result SHALL commit together.

#### Scenario: Original update resumes after restart
- **WHEN** the releasing update is reclaimed while its dialogue remains `submitting`
- **THEN** it may resume with the original idempotency authority and converge on one completed dialogue plus one result job
