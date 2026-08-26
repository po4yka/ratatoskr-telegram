## Why

The `gate` job of `ci` fails on `main` (run 32922850496) at `services/webhook/tests/boot.rs:461`: `a_listener_that_cannot_bind_exits_one` panics with "the operator was not told which listener failed". The test hardcodes `RATATOSKR__DATABASE__URL` to `postgres://telegram:telegram@127.0.0.1:15437/telegram`, a port nothing serves in CI or in the documented local setup. Because `RuntimeRole::Dispatcher` requires a database (`role_requires_database`, `crates/http/src/lib.rs:363-368`), `prepare_database` fails before the process ever reaches the bind step the test is supposed to exercise, so the process exits for the wrong reason and never logs the bind-failure message the assertion looks for.

## What Changes

- Replace the hardcoded, unreachable `127.0.0.1:15437` database literal in `a_listener_that_cannot_bind_exits_one` with a real disposable database created through `telegram_persistence::test_support::TestDatabase::create()`, the same helper every other "reachable database" case in this file already uses, wrapped in a small `tokio::runtime::Runtime` exactly as `the_webhook_boots_with_full_intake_configuration_and_reports_ready` (same file) already does for a plain `#[test]`.
- No other hardcoded or otherwise-unreachable database literal was found elsewhere in the repository; every other occurrence either intentionally targets an unreachable address to test a refusal path (`127.0.0.1:5`, nothing listens there by design) or already points at the documented default (`127.0.0.1:5432`) or a `TestDatabase`-issued URL.

## Capabilities

No product behaviour changes. This repairs a test's own fixture so the assertion it already makes about the dispatcher's bind-failure reporting is actually exercised; `skip_specs: true` is set in the change manifest.

## Impact

- `services/webhook/tests/boot.rs` (`a_listener_that_cannot_bind_exits_one`).
- No production code, wire contract, schema, or CI workflow file changes.
