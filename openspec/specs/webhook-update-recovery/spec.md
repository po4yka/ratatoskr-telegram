# webhook-update-recovery Specification

## Purpose

Defines durable webhook admission so authenticated Telegram updates remain processable across service restarts while sensitive payload data is retained only until settlement.

## Requirements

### Requirement: Admission is durable before acknowledgement

The webhook SHALL durably store the parsed update data required by its worker before it returns a successful acknowledgement.

#### Scenario: admitted update outlives the webhook process

- **WHEN** an update is accepted and the original webhook process stops before worker execution
- **THEN** a worker in a new process loads and settles the pending update without Telegram delivering it again

#### Scenario: persistence fails before acknowledgement

- **WHEN** the service cannot persist the processable update
- **THEN** the webhook returns a retryable failure and does not claim that the update was accepted

### Requirement: The database is the work authority

The worker SHALL claim processable updates from durable state; an in-memory notification SHALL NOT be the only copy or authority for accepted work.

#### Scenario: the in-memory notification is lost

- **WHEN** an accepted update has no surviving in-memory notification
- **THEN** the worker still discovers and processes the durable pending row

#### Scenario: the update is redelivered

- **WHEN** a duplicate delivery names an accepted pending update
- **THEN** the existing durable work is processed once and no duplicate row or side effect is created

### Requirement: Terminal settlement minimizes payload retention

The worker SHALL remove the processable Telegram payload in the same durable transition that records a terminal update state.

#### Scenario: processed payload is removed

- **WHEN** an update reaches processed, unsupported, or failed state
- **THEN** its row retains only minimized deduplication and audit fields and no longer contains the processable payload
