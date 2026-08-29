## ADDED Requirements

### Requirement: Dispatcher shutdown owns and drains background work

On graceful shutdown the service SHALL signal every dispatcher background worker to stop admitting new work, SHALL wait for owned worker tasks before closing shared database and telemetry resources, and SHALL bound that wait. Work that cannot reach its durable boundary before the deadline SHALL remain recoverable, and every aborted task SHALL be awaited before process exit.

#### Scenario: Graceful stop waits for an admitted delivery
- **WHEN** shutdown begins while one outbound request is awaiting its provider response
- **THEN** no later job is claimed and the admitted request may persist its known outcome before shared resources close

#### Scenario: Drain deadline expires
- **WHEN** a worker does not finish within the configured graceful interval
- **THEN** the service cancels and awaits it while leaving its durable work in a recoverable state
