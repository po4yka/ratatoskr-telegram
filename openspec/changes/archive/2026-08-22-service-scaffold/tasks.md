# Tasks: service scaffold

## 1. Workspace skeleton

- [x] 1.1 Create the workspace root files — `Cargo.toml` (members, workspace lints, shared dependencies incl. declared `teloxide`), `clippy.toml` (size limits, test allowances, env-read ban), `deny.toml`, `rust-toolchain.toml`, `rustfmt.toml`. No behavior exists to test first; configuration and generated manifests. Verify: `cargo fetch --locked && cargo metadata --format-version 1 > /dev/null` resolves.
- [x] 1.2 Create empty crate skeletons (`crates/core`, `crates/telemetry`, `crates/http`, `crates/persistence`, `services/webhook`, `services/dispatcher`) with manifests inheriting workspace lints and a compiling `lib.rs`/`main.rs`. No behavior yet. Verify: `cargo build --workspace --locked`.

## 2. Runtime roles

- [x] 2.1 Write `crates/core/tests/roles.rs`: assert the role set is exactly `{webhook, dispatcher}`, each role's `as_str`/`binary_name` is distinct, and default admin ports are distinct and outside platform's 9464–9466 block. Run it and confirm it fails because the types do not exist. (Confirmed red: `no RuntimeRole in role`.)
- [x] 2.2 Implement `crates/core/src/role.rs` until the test passes. (3 tests green.)

## 3. Typed configuration

- [x] 3.1 Write `crates/core/tests/config_sources.rs`: defaults load in an empty environment for both roles; an env var overrides one field; unknown keys are rejected naming the key; nested tables parse from `__`-joined variables. Confirm failure: no config module exists. (Confirmed red.)
- [x] 3.2 Write `crates/core/tests/config_validation.rs`: two invalid values yield a report containing both violations with no supplied value quoted; valid defaults produce no violations; violation reports never echo values. Confirm failure. (Confirmed red.)
- [x] 3.3 Implement `crates/core/src/config/` (model with `deny_unknown_fields`, figment loader under `RATATOSKR__`, validation collecting all violations V1–V8, value-free reports, exit code 78) until both tests pass. (7 config tests green.)

## 4. Error hierarchy

- [x] 4.1 Write `crates/core/tests/error_taxonomy.rs`: every subsystem renders its stable lowercase label; an internal failure carries its subsystem and source; the internal source chain never appears in `Display`. Confirm failure: no error module exists. (Confirmed red.)
- [x] 4.2 Implement `crates/core/src/error.rs` (`Subsystem`, `TelegramError::Internal`, `internal()`, `log()`) until the test passes. (3 tests green.)

## 5. Telemetry

- [x] 5.1 Write `crates/telemetry/tests/subscriber.rs`: an invalid log filter returns the filter error; initialisation twice in one process fails as already-installed; spans minted with no OTLP config carry non-zero trace ids. Confirm failure: no telemetry crate body exists. (Confirmed red.)
- [x] 5.2 Write the redaction tests (inline `#[cfg(test)]` in the crate, as platform does): an OTLP header canary reaches exporter metadata but appears in no `Debug` rendering or constructed-error message; an https endpoint builds an exporter. Confirm failure.
- [x] 5.3 Implement `crates/telemetry/src/` (identity constants, correlation `trace_id_of`, init/guard, metrics names, OTLP exporter builder) until all pass. (6 tests green.)

## 6. Persistence and schema

- [x] 6.1 Root `schema.sql` defining the first-version `telegram` schema (schema + ownership comments + conventions, no tables). Documentation artifact; verified by 6.2.
- [x] 6.2 Write `crates/persistence/tests/schema.rs` (disposable database per test): apply creates the `telegram` schema on a fresh database; applying twice succeeds changing nothing; `ping` answers; connect failures leak no credential. Requires a PostgreSQL via `TELEGRAM_TEST_DATABASE_URL`. Confirmed red before implementation.
- [x] 6.3 Implement `crates/persistence/src/` (`Database::connect/ping/apply_schema/close`, embedded schema via `include_str!`, transaction-scoped advisory lock distinct from other services) and the `test_support` module for service-level tests. (2 tests green against real PostgreSQL.)

## 7. Operator plane

- [x] 7.1 Write `crates/http/tests/admin.rs`: `/health/live` answers 200 `live` before startup completes; `/health/ready` answers 503 `not_ready` with a startup check before completion and 200 after; readiness lists a failing database check only when one is configured; `/metrics` renders Prometheus text including the build-info series; `/version` carries service/role/version/sha/toolchain; every response carries `Cache-Control: no-store`. Confirmed red before implementation.
- [x] 7.2 Implement `crates/http/src/lifecycle.rs` (`RuntimeState`, checks) and `admin.rs` until the test passes. (7 tests green.)
- [x] 7.3 Process-lifecycle behavior (config refusal exit 78, drain to exit 0 on signal, bind failure exit 1) is covered end-to-end by the binaries' boot tests rather than a separate harness file — the lifecycle runs real listeners and signals, which the boot suite exercises against the built binaries. Folded into 8.1.
- [x] 7.4 Implement `crates/http/src/shutdown.rs` (`serve`, `drain_and_close`, signal) and `run()`/`check_config` (config → telemetry → database → listener → startup-complete → drain → flush → exit code). Verified by the boot tests below.

## 8. Binaries and boot

- [x] 8.1 Write `services/webhook/tests/boot.rs`: spawn each binary, poll `/health/live` and `/health/ready` to 200, send SIGTERM, expect exit 0 with startup and graceful-shutdown log lines; plus check-config exit codes 0/78 with value-free reports, bind failure exit 1, and an unreachable configured database keeping readiness failing by name. Confirmed red (binaries were stubs).
- [x] 8.2 Wire `services/webhook/src/main.rs` and `services/dispatcher/src/main.rs` to `run(role)` + `check-config`, and make the boot tests pass. (4 tests green.)

## 9. Gate and documentation

- [x] 9.1 Add `.github/workflows/ci.yml` (PostgreSQL service container pinned by digest, `TELEGRAM_TEST_DATABASE_URL`, gate commands, DEVELOPMENT.md sync step) and the identical fenced command list under `### Rust — also the CI gate` in `DEVELOPMENT.md`; add minimal `compose.yaml` and `.env.example`.
- [x] 9.2 Update `README.md` status/layout/project-status sections and the `DEVELOPMENT.md` stage section to describe what now exists.
- [x] 9.3 Full gate locally: every command in the DEVELOPMENT.md list, in order, green.

## 10. Archive

- [x] 10.1 Tick all tasks, `openspec validate --archived` after archive, merge to `main`, push, remove worktree and branch.
