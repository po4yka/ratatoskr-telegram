## Context

See [proposal.md](proposal.md). `boot.rs` runs the shipped binaries as real processes so `DEVELOPMENT.md`'s local-run instructions cannot rot. Since plan item 4 both roles require a working PostgreSQL connection during `prepare_database`, so any test that wants to reach a *later* startup step (like the admin listener bind) must give the process a database it can actually use. The file already has a standard way to do that: `telegram_persistence::test_support::TestDatabase::create()`, which creates and schemas a disposable database and is used by `the_webhook_boots_with_full_intake_configuration_and_reports_ready`, `startup_provisions_owner_once_without_resurrection`, and (indirectly, via the fixed default) `a_webhook_whose_database_is_unreachable_refuses_to_start`. `a_listener_that_cannot_bind_exits_one` alone used a hand-written literal on port `15437` instead, which nothing serves — the port is presently also owned by an unrelated repository's disposable test database in this fleet's shared container set, so a coincidental future collision was possible on top of the outright failure.

## Goals / Non-Goals

**Goals:**

- Give `a_listener_that_cannot_bind_exits_one` a genuinely reachable database so the dispatcher process gets past `prepare_database` and reaches the admin-listener bind attempt the test is actually about.
- Use the file's own established pattern (`TestDatabase::create()` inside a small `tokio::runtime::Runtime`, as the plain, non-`tokio::test` function already does at line ~139) rather than inventing a second way to get a database URL.
- Confirm the assertion this test makes ("the operator was not told which listener failed") is still load-bearing, not vacuously true, by breaking the source message and watching the test fail for that reason.

**Non-Goals:**

- Change what the test asserts about exit codes or log content.
- Touch any other test file; the repository-wide grep for `15437` and for other database literals outside the documented `127.0.0.1:5432` default or the intentionally-unreachable `127.0.0.1:5` (used by two refusal tests on purpose) found nothing else to fix.
- Change `crates/http/src/lib.rs` bind-failure logging — it already reports correctly, which is exactly why the test's own fixture was the problem, not the code under test.

## Decisions

Replace the hardcoded connection string with a database created by `TestDatabase::create()` inside a `tokio::runtime::Runtime::new()` block, then pass `test.url()` to the child process's `RATATOSKR__DATABASE__URL`. The created `TestDatabase` value is allowed to be dropped without an explicit `cleanup()` call, matching `the_webhook_boots_with_full_intake_configuration_and_reports_ready`'s existing pattern in the same file: the test only needs a URL the dispatcher can connect to during preparation, never inspects rows in it afterward, and `test_support.rs` documents that a database left behind is acceptable scaffolding, not a defect.

## Risks / Trade-offs

- [A disposable database is now created and never explicitly dropped for this test] → Accepted; it mirrors an existing, reviewed pattern in the same file, and the whole point of a per-test disposable database is that leaving one behind never affects another test.
- [The fix could make the assertion vacuously true if the source message text also drifted] → Verified directly: temporarily changing the bind-failure log message in `crates/http/src/lib.rs` and rebuilding reproduces a failure at the "operator was not told which listener failed" assertion specifically, confirming it still exercises real behaviour; the change was reverted before committing.

## Migration Plan

Test-only change with no production code, schema, or CI workflow edits. Land the fix; the same `gate` job that failed on run 32922850496 is expected to pass without any repeated toolchain, service, or workflow change.
