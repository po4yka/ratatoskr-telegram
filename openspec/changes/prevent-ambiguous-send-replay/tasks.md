## 1. Atomic known-acknowledgement settlement

- [x] 1.1 RED: add acknowledged_delivery_updates_job_binding_and_revision_atomically to crates/persistence/tests/outbound.rs with a temporary statement fault; predict current independent writes expose partial acknowledgement state, run only this test through build-gate -- cargo nextest run --locked -p ratatoskr-telegram-persistence acknowledged_delivery_updates_job_binding_and_revision_atomically, and confirm the partial-state assertion fails
- [x] 1.2 GREEN: edit the current schema for explicit unknown outcome and implement one payload-aware transaction covering job settlement, provider message identity, binding, render, callback, and notification effects; rerun the exact test until green
- [x] 1.3 REGRESSION: add known_ack_retries_local_recording_without_a_second_wire_call using a hand-written acknowledgement-store fake with a one-shot fault; the seam and behavior landed together during parallel work, so no false RED claim is made; run the exact test and observe one wire call plus two local recording attempts
- [x] 1.4 GREEN: retain a successful Bot API response through bounded local transaction retries inside the admitted delivery critical section and never call Telegram again for that known acknowledgement; rerun the exact test until green

## 2. Honest ambiguous-outcome recovery

- [x] 2.1 RED: add expired_send_with_unknown_outcome_is_quarantined_without_a_second_wire_call using a stale sending row and fake Bot API recorder; predict current reclaim performs another send, run the exact test through the build gate, and confirm the extra call
- [x] 2.2 GREEN: differentiate stale send from edit recovery, atomically transition non-idempotent stale sends to outcome unknown, and keep them ineligible for ordinary claims; rerun the exact test until green
- [x] 2.3 RED: add transport_timeout_enters_outcome_unknown_without_retry covering a timeout after request transmission; predict current code schedules an ordinary retry, run the exact test through the build gate, and confirm the retry-state assertion fails
- [x] 2.4 GREEN: classify transport and process uncertainty as unknown while preserving the existing bounded retry tests for provider-confirmed not-applied responses, then rerun the exact test and that regression set until green
- [x] 2.5 RED: add operator_inspection_reports_unknown_without_private_content; predict current operator projection has no unknown class, run the exact operator test through the build gate, and confirm the missing-class assertion
- [x] 2.6 GREEN: expose bounded unknown-outcome inspection and explicit execute-only recovery with duplicate-risk warning, then rerun the exact test until green
- [x] 2.7 REVIEW FIX: fence every settlement by the claimed attempt so an expired edit's stale worker cannot overwrite its reclaimer; cover the A/B race in persistence
- [x] 2.8 REVIEW FIX: prevent explicit recovery from regressing a newer binding, preserve and retarget notification decision authority, and omit arbitrary correlation text from operator inspection

## 3. Regression gate

- [x] 3.1 REFACTOR: share acknowledgement transaction dispatch without changing edit semantics, then run persistence and dispatcher crate suites through the build gate
- [x] 3.2 Run the complete DEVELOPMENT.md gate, strict validation for this change, and diff review for automatic ambiguous replay, private payload exposure, migrations, or unrelated changes
- [x] 3.3 Mark tasks complete only after observed checks, stage only this change's paths, inspect the staged diff, and create its dedicated main-branch commit
