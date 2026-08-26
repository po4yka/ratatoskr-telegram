## 1. Reproduce the failure

- [x] 1.1 Observe `a_listener_that_cannot_bind_exits_one` fail before the change because the hardcoded database URL (`127.0.0.1:15437`) is unreachable, so `prepare_database` fails before the bind step the test asserts about; this is a test-fixture bug, so the already-failing CI test is the failing test rather than a new unit test. Observed in `ci` run 32922850496, job `gate`: `thread 'a_listener_that_cannot_bind_exits_one' panicked at services/webhook/tests/boot.rs:461:5: the operator was not told which listener failed`. Reproduced locally the same way: `cargo test -p ratatoskr-telegram-webhook --test boot -- a_listener_that_cannot_bind_exits_one --exact` fails identically against the unmodified test.

## 2. Fix the fixture

- [x] 2.1 Replace the hardcoded, unreachable database literal with a real disposable database from `telegram_persistence::test_support::TestDatabase::create()`, wrapped in a `tokio::runtime::Runtime` exactly as the same file's `the_webhook_boots_with_full_intake_configuration_and_reports_ready` already does for a plain `#[test]`.
- [x] 2.2 Re-run the same single test and observe it pass: `cargo test -p ratatoskr-telegram-webhook --test boot -- a_listener_that_cannot_bind_exits_one --exact` — `test result: ok. 1 passed`.

## 3. Confirm the assertion is still load-bearing

- [x] 3.1 Temporarily change the bind-failure log message in `crates/http/src/lib.rs` (`"the admin listener could not bind"` to a deliberately different string), rebuild, and re-run the test: it fails at the `"the operator was not told which listener failed"` assertion specifically (exit code still 1, only the message-content assertion fails), confirming the assertion actually exercises the logged text rather than being vacuously satisfied. Reverted the source change immediately afterward; `git status` shows no diff to `crates/http/src/lib.rs`.

## 4. Sweep for the same class of bug

- [x] 4.1 Grep the whole repository for `15437` and for any other `postgres://...@127.0.0.1:PORT` literal outside the documented default (`5432`); found no other occurrence. The two other non-`5432`, non-`TestDatabase` literals in the repository (`postgres://nobody:nope@127.0.0.1:5/nowhere`, in `crates/persistence/tests/schema.rs` and twice in `services/webhook/tests/boot.rs`) are deliberately unreachable — they test refusal-on-unreachable-database paths and are documented as such in an adjacent comment — and are correct as written.

## 5. Verify the full local gate

- [x] 5.1 Run the full documented gate from `DEVELOPMENT.md` in order (`cargo fetch --locked`, `cargo deny check`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, the tracked-file-length check, `cargo build --workspace --locked`, `cargo test --workspace --locked`, `cargo build --workspace --locked --release`) and observe every step pass.
- [ ] 5.2 Observe the hosted `ci` / `gate` job pass on the pushed commit.
