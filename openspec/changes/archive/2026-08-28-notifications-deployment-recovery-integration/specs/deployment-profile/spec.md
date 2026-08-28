## Purpose

Defines the production-shaped systemd profile and structural evidence for running Telegram's webhook and dispatcher roles on Ratatoskr's constrained single host.

## ADDED Requirements

### Requirement: One systemd unit owns each runtime role

The deployment SHALL provide separate webhook and dispatcher units using the release binaries, role-specific configuration, and the workspace port allocations: webhook public `8182`, webhook operator `9467`, and dispatcher operator `9468`. The dispatcher SHALL have no public listener.

#### Scenario: Unit files are validated structurally

- **WHEN** the deploy validation examines both unit files without starting a host service
- **THEN** each unit selects exactly one binary role and all listener values match the workspace allocations

### Requirement: Units enforce the single-host safety envelope

Each unit SHALL use `Type=exec`, `TimeoutStopSec=130s`, bounded restart/start-limit settings, explicit resource ceilings, least-privilege service identity, no-new-privileges and filesystem hardening, NVMe-backed append-only service logs with rotation, and explicit dependency ordering. Secrets SHALL be loaded from root-owned credential files and SHALL NOT appear in unit files, checked-in environment examples, command output, or logs.

#### Scenario: Required safety directives are absent

- **WHEN** a unit omits or weakens a required supervision, resource, logging, credential, or hardening directive
- **THEN** structural validation fails naming the missing property

### Requirement: Role startup reflects dependencies truthfully

The webhook SHALL start only after its configured database and public-listener prerequisites are available. The dispatcher SHALL start only after the current Telegram schema is ready and its database, Bot API credential, and notification bus credential are readable. Neither role SHALL claim readiness while a required dependency is unavailable.

#### Scenario: Dispatcher bus credential is unreadable

- **WHEN** the dispatcher starts with a configured but unreadable notification-consumer credential
- **THEN** it refuses startup or remains not ready with a safe dependency class and sends no notifications

### Requirement: Deployment validation does not mutate the target

Repository validation SHALL parse and inspect deploy artifacts without installing units, changing firewall or tunnel state, registering a webhook, rotating a secret, or contacting a real Telegram chat.

#### Scenario: Deploy gate runs on a development machine

- **WHEN** the structural deploy gate passes outside the target host
- **THEN** the evidence states deploy-artifact validity and does not claim live deployment
