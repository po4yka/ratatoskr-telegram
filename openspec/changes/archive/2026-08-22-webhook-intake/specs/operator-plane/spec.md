## Purpose

The admin listener every Ratatoskr Telegram binary serves: liveness, readiness, metrics and version, and the graceful stop behind them.

## MODIFIED Requirements

### Requirement: Readiness reflects configured dependencies truthfully

Readiness SHALL include a database check only when a database is configured: a process without one SHALL report no database check rather than a passing or failing one. A database check SHALL reflect the most recent background probe, and the probe SHALL run in the background so a readiness request never opens a connection. A role whose routes write through the database SHALL treat an unreachable configured database as a startup failure — refusing to start rather than binding listeners that cannot serve their purpose; roles with no such route SHALL keep reporting the failing check while staying up.

#### Scenario: No database configured means no database check

- **WHEN** a process starts without `RATATOSKR__DATABASE__URL`
- **THEN** `/health/ready` lists no database check

#### Scenario: A configured database appears as a check

- **WHEN** a role that may run without database routes starts with an unreachable `RATATOSKR__DATABASE__URL`
- **THEN** `/health/ready` reports 503 with the database check failing and reason `dependency_unavailable`

#### Scenario: The webhook refuses to start without its database

- **WHEN** the webhook role starts with a database it cannot reach, or none configured
- **THEN** the process logs the failure class once and exits 1 without serving updates
