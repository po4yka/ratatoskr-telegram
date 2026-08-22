# operator-plane Specification

## Purpose
The admin listener every Ratatoskr Telegram binary serves: liveness, readiness, metrics and version, and the graceful stop behind them.

## Requirements

### Requirement: The operator plane serves four endpoints on the admin listener only

Each binary SHALL serve `GET /health/live`, `GET /health/ready`, `GET /metrics` and `GET /version` on its admin listener. Every admin response SHALL carry `Cache-Control: no-store`. The admin plane SHALL NOT wrap failures in a client error envelope; a failing readiness check names the check.

#### Scenario: Liveness answers before startup completes
- **WHEN** `/health/live` is polled while the process is still starting
- **THEN** it answers 200 with body state `live` and the role name

#### Scenario: Readiness fails until startup completes
- **WHEN** `/health/ready` is polled after the listeners are bound but before startup is marked complete
- **THEN** it answers 503 with body state `not_ready` and a named failed check

#### Scenario: Readiness passes once startup completes
- **WHEN** `/health/ready` is polled after startup completes
- **THEN** it answers 200 with body state `ready`

#### Scenario: Metrics renders Prometheus text
- **WHEN** `/metrics` is scraped
- **THEN** it returns Prometheus exposition text including the build info series

#### Scenario: Version reports the build identity
- **WHEN** `/version` is fetched
- **THEN** it reports the service name, the role, the crate version, the git SHA (or `unknown` when unset) and the toolchain version

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

### Requirement: Shutdown drains before closing

On SIGTERM or SIGINT the process SHALL mark itself not ready immediately, keep serving for the drain window, then close each listener and let in-flight work finish within the grace window, then flush telemetry and exit 0. A second signal SHALL short-circuit the wait. Liveness SHALL answer throughout.

#### Scenario: Clean stop on SIGTERM
- **WHEN** a running process receives SIGTERM
- **THEN** readiness answers 503 during the drain, the process exits 0, and liveness kept answering until the listener closed
