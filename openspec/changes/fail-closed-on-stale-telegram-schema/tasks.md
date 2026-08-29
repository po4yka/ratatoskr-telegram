## 1. Exact schema identity

- [x] 1.1 RED: add existing_schema_without_current_fingerprint_is_rejected in crates/persistence/tests/schema.rs; predict that the error assertion fails because current apply_schema returns success for a manually created Telegram namespace, then run only this test through build-gate -- cargo nextest run --locked -p ratatoskr-telegram-persistence existing_schema_without_current_fingerprint_is_rejected and confirm that assertion failure
- [x] 1.2 GREEN: add deterministic embedded-schema hashing, create and populate singleton schema-match authority inside fresh all-or-nothing application, return a typed safe error for missing authority, and rerun the exact test until green
- [x] 1.3 RED: add different_schema_fingerprint_is_rejected_by_verify; create the current tables with deliberately different match evidence, predict current verification returns success, then run only this test through the build gate and observe that assertion fail
- [x] 1.4 GREEN: make dispatcher verification compare exact stored and embedded digests under the startup lock, leave mismatched storage unchanged, and rerun the exact test until green

## 2. Recovery contract and verification

- [x] 2.1 Document the safe stale-schema error and disposable development-database recreation workflow in the existing development documentation; no RED test applies because this is static operator guidance, then verify every named command and path against the implemented interface
- [x] 2.2 REFACTOR: share digest comparison and safe error mapping between application and verification without changing behavior, then run the persistence crate tests through build-gate -- cargo nextest run --locked -p ratatoskr-telegram-persistence
- [x] 2.3 Run the complete DEVELOPMENT.md gate through the required machine-wide build gate where commands compile, run openspec validate fail-closed-on-stale-telegram-schema --strict, and inspect the diff for migration/version artifacts or unrelated changes
- [x] 2.4 Mark these tasks complete only after observing the checks, stage only this change's code, tests, docs, and OpenSpec paths, inspect the staged diff, and create its dedicated main-branch commit with no unrelated work
