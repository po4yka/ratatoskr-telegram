## 1. Atomic accepted projection

- [x] 1.1 RED: add accepted_capture_projection_is_all_or_nothing to the PostgreSQL-backed capture tests using a temporary trigger that rejects acknowledgement insertion; predict the current path leaves a partial binding or intent, run only this test through build-gate -- cargo nextest run --locked -p ratatoskr-telegram-webhook accepted_capture_projection_is_all_or_nothing, and confirm the predicted assertion failure
- [x] 1.2 GREEN: implement one persistence transaction that resolves or inserts the binding and lets only its winner insert the opaque intent and acknowledgement job, then rerun the exact test until green
- [x] 1.3 RED: add accepted_capture_retries_projection_after_storage_failure with a one-shot storage fault and a fake Platform call recorder; predict the current worker terminally fails and minimizes the update after the first acceptance, then run only this test through the build gate and observe that failure
- [x] 1.4 GREEN: introduce an explicit recoverable handler and worker outcome after known acceptance, retain the processable payload, reclaim it with the same Platform idempotency key, and rerun until the update becomes processed with one binding, intent, and acknowledgement job

## 2. Convergence, leasing, and bounded recovery

- [x] 2.1 REGRESSION: add concurrent_accepted_capture_recovery_enqueues_one_acknowledgement and run the exact test through the build gate; because task 1.2 had already replaced the independent writes, the new test was immediately green rather than manufacturing a false RED
- [x] 2.2 GREEN: verify the task 1.2 binding-row lock and existence reconciliation enforce one transaction winner for concurrent and unknown-commit recovery; the exact concurrent test passed with one binding, intent, and acknowledgement
- [x] 2.3 RED: add claimed_update_is_not_reclaimed_before_lease_expiry and observe the immediate second claim incorrectly return the same processing update
- [x] 2.4 GREEN: add a durable processing lease and rerun the exact persistence test until the immediate second claim returns none
- [x] 2.5 RED: rerun accepted_capture_retries_projection_after_storage_failure after leasing and observe the retained update miss its recovery deadline without an explicit retry schedule
- [x] 2.6 GREEN: release recoverable work to an accepted state with a bounded retry time, then rerun until one-shot recovery processes under the original idempotency key
- [x] 2.7 REGRESSION: add persistent_capture_projection_fault_does_not_starve_later_update and verify the lease and retry schedule let a newer update settle while the faulted capture retains its payload
- [x] 2.8 RED: add expired_operation_intent_is_replaced_during_capture_projection_recovery and observe zero live intents when only an expired historical token exists
- [x] 2.9 GREEN: reconcile only against live unconsumed operation intents and rerun until recovery leaves one fresh authority
- [x] 2.10 RED: add recoverable_update_dead_letters_after_bounded_attempts and observe the eighth failed recovery remain automatically claimable
- [x] 2.11 GREEN: persist claim attempts and move exhausted work to recovery_required with its authenticated payload retained and no further automatic claim

## 3. Regression gate

- [x] 3.1 REFACTOR: keep Platform HTTP outside PostgreSQL transactions and consolidate only local projection plumbing, then run webhook and persistence crate suites through the build gate
- [x] 3.2 Run the complete DEVELOPMENT.md gate, strict validation for this change, and diff review for retained private payloads, new dependencies, or cross-repository contract drift
- [x] 3.3 Mark tasks complete only after observed checks, stage only this change's paths, inspect the staged diff, and create its dedicated main-branch commit
