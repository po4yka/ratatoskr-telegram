## 1. Durable confirmation recovery authority

- [x] 1.1 RED: add a persistence test submitting_dialogue_records_releasing_update proving the winning confirm transition stores its bot and update identity and a different update cannot replace it; predict current state has no such authority, run only this test through build-gate -- cargo nextest run --locked -p ratatoskr-telegram-persistence submitting_dialogue_records_releasing_update, and confirm the expected assertion failure
- [x] 1.2 GREEN: edit the current schema in place, pass admitted update identity into confirmation consumption, and persist it atomically with the submitting transition; rerun the exact test until green
- [x] 1.3 RED: add foreign_update_cannot_resume_consumed_confirmation; predict current consumed-token path has no same-update recovery distinction, run only this test through the build gate, and confirm the predicted behavior failure
- [x] 1.4 GREEN: allow resume only for the recorded durable update while preserving common refusal for foreign, replayed, expired, and stale callbacks, then rerun the exact test until green

## 2. Atomic result projection

- [x] 2.1 REGRESSION: add confirmed_action_result_completion_is_atomic using a database trigger that rejects result-job insertion; the transaction implementation landed in parallel before the test compiled, so record the test as immediately green rather than manufacture a false RED
- [x] 2.2 GREEN: implement one transaction that validates the releasing update, completes the dialogue, and enqueues the structured result, then rerun the exact test until green
- [x] 2.3 REGRESSION: add confirmed_action_recovers_result_after_storage_failure with one-shot storage failure and recorded Platform idempotency keys; the recovery implementation landed in parallel before the test compiled, so record the exact test as immediately green
- [x] 2.4 GREEN: return a recoverable worker outcome after confirmation, retain the payload, resume under the same action identity, and rerun until exactly one completed dialogue and result job exist and every Platform request key is identical
- [x] 2.5 RED: add permanent_action_refusal_settles_once_with_safe_result after review found permanent Platform refusals entering the recovery budget; observe eight action calls instead of one
- [x] 2.6 GREEN: classify only uncertain failures as recoverable, atomically complete an explicit refusal with a safe no-success projection, and rerun the exact test until green

## 3. Regression gate

- [x] 3.1 REFACTOR: reuse transaction-aware outbound insertion without holding a database transaction over Platform HTTP, then run webhook and persistence crate suites through the build gate
- [x] 3.2 Run the complete DEVELOPMENT.md gate, strict validation for this change, and diff review for callback authority expansion, provider credentials, migration artifacts, or unrelated work
- [x] 3.3 Mark tasks complete only after observed checks, stage only this change's paths, inspect the staged diff, and create its dedicated main-branch commit
