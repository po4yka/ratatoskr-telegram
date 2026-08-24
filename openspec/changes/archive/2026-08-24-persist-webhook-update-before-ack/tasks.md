## 1. Restart-safe update processing

- [x] 1.1 Add `an_admitted_update_is_processed_after_worker_restart` to `services/webhook/tests/intake.rs` and run it RED; after dropping the original in-memory receiver, the restarted worker must time out because no durable processable payload exists
- [x] 1.2 Persist the parsed update in the current `schema.sql`, make PostgreSQL the worker's claim/load authority with the channel as a wake-up hint, and run `an_admitted_update_is_processed_after_worker_restart` GREEN

## 2. Payload minimization after settlement

- [x] 2.1 Add `terminal_settlement_removes_the_processable_payload` to `crates/persistence/tests/updates.rs` and run it RED; the persisted payload must still be present after the current terminal state update
- [x] 2.2 Clear the processable payload atomically with every terminal settlement while retaining deduplication fields, then run `terminal_settlement_removes_the_processable_payload` GREEN

## 3. Repository verification

- [x] 3.1 Run the persistence and webhook crate tests against fresh PostgreSQL 17, `cargo fmt --all -- --check`, and the repository Clippy gate; this broad verification follows the green behavior tests and adds no behavior
- [x] 3.2 Run the complete gate documented in `DEVELOPMENT.md`, inspect the final diff for payload exposure or secret logging, and verify only intended paths are changed; this verification task adds no behavior

Validation note: the persistence and webhook suites, dependency policy, formatting, line limit,
Clippy gate, debug build, complete workspace test suite, and release build pass. The inherited
Clippy violations were corrected directly without lint suppression or policy changes.
