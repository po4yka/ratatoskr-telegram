# Service scaffold: typed configuration, telemetry, operator plane, errors, and the `telegram` schema

## Why

The repository is architecture bootstrap: it holds intent documents and a gate but no code. Every
later plan item (Bot API webhook, dispatcher, identity binding, Mini App auth) needs the same
foundation the sibling Ratatoskr services already run on — a Rust workspace whose process refuses to
start on bad configuration, emits correlated structured telemetry, answers operator probes, and owns
its `telegram` database schema. Building that foundation once, following the fleet conventions, is
plan item 1 of `docs/IMPLEMENTATION_PLAN.md`.

## What Changes

- Add the Rust workspace: `crates/core`, `crates/telemetry`, `crates/http`, `crates/persistence`,
  and the two planned deployables `services/webhook` and `services/dispatcher`, with the fleet's
  root configuration (`clippy.toml` carrying the size limits, `deny.toml`, `rust-toolchain.toml`,
  `rustfmt.toml`, workspace-level lints).
- Add typed configuration loaded from `RATATOSKR__` environment variables with `deny_unknown_fields`
  and startup validation that collects every violation; a process exits `78` (`EX_CONFIG`) rather
  than starting wrong. Configuration is read in exactly one module.
- Add structured telemetry: JSON or pretty logs with real W3C trace ids even with no collector,
  optional OTLP span export, a Prometheus recorder, and build identity (`service.name`,
  version, git SHA, toolchain).
- Add the typed error hierarchy: a two-arm boundary error (client-visible rejections vs internal
  failures that never leak their source), crate-level `ConfigError`, `TelemetryError` and
  `PersistenceError`, and bounded `Subsystem` telemetry labels.
- Add the operator plane on a per-role admin listener: `/health/live`, `/health/ready`,
  `/metrics`, `/version`, with readiness computed from startup, drain and configured-dependency
  checks, and the drain-then-close-then-flush shutdown sequence on SIGTERM/SIGINT.
- Add `schema.sql` defining the first-version `telegram` schema (schema created; tables arrive with
  the features that own them), applied idempotently at startup when a database is configured, and
  the readiness database check backed by a live probe.
- Add the test harness: configuration validation tests, operator-plane tests, telemetry tests, a
  schema integration test against a disposable PostgreSQL database, and a boot test that starts each
  binary, polls the operator plane and stops it on a signal.
- Add `.github/workflows/ci.yml` (the gate) and the matching command list in `DEVELOPMENT.md`;
  `compose.yaml` for the local PostgreSQL the integration tests need.
- Declare `teloxide` as the workspace Bot API dependency for the next plan item; no Bot API client,
  webhook route, or Telegram network call is built here.
- Update `README.md` and `DEVELOPMENT.md` status sections to describe what exists.

## Capabilities

### New Capabilities

- `service-configuration`: how a process reads, validates, reports and refuses configuration.
- `operator-plane`: the admin listener's liveness, readiness, metrics and version endpoints, and
  the shutdown sequence behind them.
- `telemetry`: log formats, trace correlation without a collector, OTLP export, metrics recording.
- `persistence-schema`: the `telegram` schema, its application at startup, and the database
  readiness check.

### Modified Capabilities

None. `openspec/specs/` is empty by design; this is the first change.

## Impact

- New code: the five crates and two binaries above; no existing code changes.
- New gate: `.github/workflows/ci.yml` must stay green; `fleet.yml`'s "code cannot land without a
  gate / size limits" assertions become load-bearing.
- `DEVELOPMENT.md` gains the gate command list (kept identical to `ci.yml` by a workflow step).
- `README.md` status moves from "nothing is implemented" to the scaffold being in place.
- Dependencies added: tokio, axum, figment, secrecy, sqlx, tracing stack, opentelemetry, metrics,
  jiff, thiserror, serde, uuid, url, teloxide (declared, unused until the Bot API item).
- Operator ports `9467` (webhook) and `9468` (dispatcher) are taken for this service; the shared
  `ratatoskr-workspace/docs/DEPLOYMENT_TARGET.md` port table should record them when this service
  reaches deployment.
