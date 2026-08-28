# operational-recovery Specification

## Purpose
Defines executable, fail-closed procedures for rotating Telegram credentials and recovering owned stuck work without bypassing Platform or exposing private payloads.

## Requirements

### Requirement: Credential rotation is staged and reversible

The webhook-secret and bot-token runbooks SHALL validate a replacement from a root-owned file, show a redacted dry-run plan, install it atomically, restart only the affected role, verify readiness, and retain a bounded rollback path. Webhook registration or provider token revocation SHALL be an explicit separately authorized external write.

#### Scenario: Webhook secret rotation dry-run

- **WHEN** an operator runs the documented webhook-secret rotation in dry-run mode with a readable candidate file
- **THEN** the command reports the files, role, readiness checks, and external registration step without changing a file, process, database row, or Telegram webhook

#### Scenario: Candidate credential is invalid

- **WHEN** a candidate file is missing, unreadable, empty, or violates configured length requirements
- **THEN** rotation stops before replacing the active credential or restarting a role

### Requirement: Session recovery respects Platform ownership

The session runbook SHALL inspect Telegram binding and assertion evidence locally but SHALL use Platform's authorized session inspection/revocation surface for Platform sessions. It SHALL NOT write Platform tables or treat Telegram `initData`, assertions, or bot credentials as reusable sessions.

#### Scenario: Operator investigates a failed Mini App session

- **WHEN** the documented session inspection is run for a correlation reference
- **THEN** it reports only bounded local assertion outcome and directs any Platform revocation through Platform authority without printing raw auth material

### Requirement: Stuck Telegram work can be inspected before recovery

The recovery tooling SHALL provide read-only inspection for leased or retrying updates, interaction work, operation followers/projections, notification decisions, and outbound jobs. A mutation SHALL require an explicit execute flag, re-check the expected current state transactionally, and affect only service-owned retry/lease state; it SHALL NOT change a Platform operation's domain status.

#### Scenario: Stale outbound lease is recovered

- **WHEN** an operator first inspects a job whose lease expired and then executes the documented recovery with its expected identity and state
- **THEN** the lease returns to an eligible retry state exactly once without duplicating a completed delivery

#### Scenario: Operation is still running in Platform

- **WHEN** a Telegram projection appears stuck but Platform reports the operation running
- **THEN** recovery resumes or requeues only the Telegram follower/projection and leaves Platform operation state unchanged

### Requirement: Dead updates and deliveries are inspectable without private content

The runbook SHALL enumerate dead update and outbound records by bounded identifiers, timestamps, attempt count, safe failure class, and correlation reference. Default output SHALL exclude raw update payloads, message bodies, titles, usernames, chat identifiers, credentials, and provider diagnostics.

#### Scenario: Operator inspects dead updates

- **WHEN** the documented read-only command lists dead updates
- **THEN** every row contains only the bounded diagnostic projection and the command performs no mutation

### Requirement: Every runbook command is tested as written

Repository validation SHALL execute shell syntax and deterministic dry-run/read-only paths using synthetic files, processes, and database fixtures. A command requiring live provider or host authority SHALL stop before that step and state the missing authority rather than fabricating success.

#### Scenario: Runbook dry-run suite executes

- **WHEN** the repository gate runs the runbook validation suite
- **THEN** every copied command parses and its dry-run assertions pass without production credentials, host mutation, or network access
