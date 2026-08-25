## 1. Schema and persistence repositories

- [x] 1.1 RED: add `crates/persistence/tests/outbound.rs` with catalog test `message_bindings_outbound_jobs_and_inbox_exist_with_expected_shape` asserting `telegram.message_bindings`, `telegram.outbound_jobs`, `telegram.inbox` exist with their columns, CHECK vocabularies (`send_message`/`edit_message_text`; §18.1 job-state tokens), app-minted UUID PKs without DEFAULT, and timestamptz columns; confirm it fails on missing relations before touching `schema.sql`
- [x] 1.2 GREEN: declare the three tables in root `schema.sql`; rerun until 1.1 passes
- [x] 1.3 RED: add binding-repository tests in `crates/persistence/tests/outbound.rs`: `binding_is_created_and_found_by_operation` (insert + fetch round trip), `ensure_send_binding_is_idempotent_on_repeat_ack` (re-ack of same operation/chat keeps one row and updates message id), and `last_rendered_revision_advances_monotonically` (a conditional update naming an older revision is a no-op)
- [x] 1.4 GREEN: implement `crates/persistence/src/message_bindings.rs` (create-on-ack upsert, find by operation, guarded revision/state advances); rerun until 1.3 passes
- [x] 1.5 RED: add job-repository tests in `crates/persistence/tests/outbound.rs`: `enqueue_marks_ready_and_persists_payload_hash` (enqueued job is claimable, hash stored), `claim_returns_strict_per_chat_fifo_heads_only` (seed interleaved chats A,B,A,C → claims yield exactly A-head, B, C order with no second A until A's first settles), `supersede_marks_stale_ready_jobs_without_touching_in_flight` (older revision ready → superseded; sending job untouched), and `lease_expiry_reclaims_sending_rows_after_ttl` (stale lease reclaimed, fresh lease not)
- [x] 1.6 GREEN: implement `crates/persistence/src/outbound_jobs.rs` (enqueue, DISTINCT ON per-chat head claim with SKIP LOCKED + lease stamp, supersede sweep, retry reschedule, terminal settlement, dead-letter); rerun until 1.5 passes
- [x] 1.7 RED: add inbox tests in `crates/persistence/tests/inbox.rs`: `record_event_accepts_once_then_reports_duplicate` (second insert-or-check of same event_id reports duplicate) and `inbox_row_is_transactional_with_no_side_effects_on_conflict`
- [x] 1.8 GREEN: implement `crates/persistence/src/inbox.rs`; rerun until 1.7 passes

## 2. Outbound policy logic

- [x] 2.1 RED: add `services/dispatcher/src/outbound/limiter.rs` unit tests `global_bucket_refuses_calls_beyond_budget_per_window` and `per_chat_interval_enforces_minimum_gap` asserting a fake clock sees excess calls denied and same-chat calls spaced at least the configured gap while different chats proceed
- [x] 2.2 GREEN: implement the limiter over the injected `Clock` trait (global token budget + per-chat last-call timestamp); rerun until 2.1 passes
- [x] 2.3 RED: add classification tests `services/dispatcher/src/outbound/classify.rs` pinning every table row of design D5: network error → transient; `message is not modified` description → success no-op; `Forbidden: bot was blocked by the user` / `chat not found` / `message can't be edited` / `message to edit not found` / invalid-markup / migrated-to-supergroup descriptions → permanent; 429 → rate-limited carrying retry_after
- [x] 2.4 GREEN: implement the pure classifier from `BotApiError` + description matching; rerun until 2.3 passes

## 3. Outbound sender worker

- [x] 3.1 RED: add `services/dispatcher/tests/delivery.rs` with a hand-written `FakeBotApi` implementing a narrow sender seam (records chat/kind/text/timestamps; fault queue for outcomes) and test `sender_delivers_one_chat_fifo_under_concurrency` — enqueue A1..A3 plus B/C jobs, run two sender tasks concurrently against the fake, assert Bot API observes A1 before A2 before A3 and never two A jobs overlapped
- [x] 3.2 GREEN: implement the sender loop (claim head → eligibility by next_attempt_at vs injected clock → limiter gate → Bot API call → settlement per classifier); rerun until 3.1 passes
- [x] 3.3 RED: extend `services/dispatcher/tests/delivery.rs`: `rate_limited_answer_reschedules_job_and_cools_chat` (fake answers 429 once with retry_after=30s; advanced clock proves no earlier reattempt), `transient_failure_backs_off_then_dead_letters_at_bound` (fault queue returns Network errors; attempt count equals configured bound then state failed_permanent), `permanent_failure_settles_once_without_retry` (blocked-by-user answer → exactly one call), `not_modified_answer_settles_sent_and_advances_revision` (edit answered not-modified → sent + revision advanced, no retry), and `stale_edit_superseded_before_the_wire` (revision 4 claimed after revision 5 delivered → marked superseded, zero extra API calls)
- [x] 3.4 GREEN: wire retry/reschedule/dead-letter/not-modified/supersede paths into the sender using the classifier and repository primitives; rerun until 3.3 passes
- [x] 3.5 RED: add `send_ack_creates_binding_only_after_success` and `permanent_edit_failure_unbinds_and_next_revision_resends` in `services/dispatcher/tests/delivery.rs` asserting no binding exists mid-flight and that after unbind the following revision produces a sendMessage (not editMessageText) followed by a rebound message id
- [x] 3.6 GREEN: implement binding establishment on ack and unbind-and-resend fallback; rerun until 3.5 passes

## 4. Operation projection consumer

- [ ] 4.1 RED: add `services/dispatcher/src/projection/consumer.rs` unit tests with typed snapshot fixtures: `duplicate_envelope_event_id_is_dropped_and_counted`, `post_terminal_events_are_dropped_exactly_once_counted` (two succeeded snapshots → one terminal job), `stale_occurred_at_is_dropped_without_effect`, and `unbound_operation_produces_no_traffic`
- [ ] 4.2 GREEN: implement the accept step (transactional inbox insert-or-ignore → terminal check-and-set → staleness → revision assignment → enqueue edit with `next_attempt_at = max(now, last_rendered_at + render_interval)`); rerun until 4.1 passes
- [ ] 4.3 RED: add renderer tests `services/dispatcher/src/projection/render.rs`: `status_branches_drive_display_not_stage_vocabulary` and `hostile_stage_error_and_warning_text_renders_escaped_and_truncated` (stage `<b>x</b> & <script>` plus a 5000-char error message produce escaped entities, status-led text, and ≤ Telegram length bound)
- [ ] 4.4 GREEN: implement the HTML-escaping status-driven renderer; rerun until 4.3 passes
- [ ] 4.5 RED: add `progress_burst_yields_at_most_one_eligible_edit_per_interval` in `services/dispatcher/tests/projection.rs`: feed ten ticks within one simulated second through the seam into a real database, advance the injected clock across two interval windows, assert eligible-edit counts per window and that superseded intermediates never reach the recorded seam calls
- [ ] 4.6 GREEN: adjust throttle arithmetic only if 4.5 exposes a defect; rerun until 4.5 passes

## 5. Configuration, lifecycle, telemetry wiring

- [ ] 5.1 RED: add parse/validation tests in `crates/core/src/config/model.rs` + `validate.rs`: `dispatcher_section_parses_with_defaults_and_unknown_keys_refused`, `dispatcher_limits_refuse_zero_or_negative_values` (V-rule text naming each field), and `webhook_role_ignores_dispatcher_defaults_while_dispatcher_requires_database_url` (dispatcher without DATABASE__URL yields a violation naming it)
- [ ] 5.2 GREEN: add `DispatcherConfig` with defaults and validation rules; rerun until 5.1 passes
- [ ] 5.3 RED: update `services/webhook/tests/boot.rs` expectations: rewrite `the_dispatcher_boots_on_its_documented_defaults_and_reports_ready` into `the_dispatcher_requires_a_database_configuration_to_start` (absent DB config exits non-zero with the violation named) and adapt `a_dispatcher_with_an_unreachable_database_keeps_readiness_failing_and_names_the_check` to expect startup refusal; confirm both fail before the lifecycle change
- [ ] 5.4 GREEN: flip `role_requires_database` to include `Dispatcher`; rerun until 5.3 passes
- [ ] 5.5 RED: add metric-vocabulary test `delivery_outcomes_are_countable_without_content` in `services/dispatcher/tests/delivery.rs` asserting the exposition contains `telegram_delivery_retries_total{class=...}`, `telegram_rate_limit_waits_total`, `telegram_delivery_failures_total{class=...}` after driving one transient and one permanent outcome, and that no chat id or text appears anywhere in the exposition
- [ ] 5.6 GREEN: add telemetry constants and instrument limiter/sender/consumer with class-only labels; rerun until 5.5 passes

## 6. Startup factory and process integration

- [ ] 6.1 Implement `services/dispatcher/src/lib.rs` + `build.rs` mirroring the webhook: connect database, spawn sender and consumer workers on the shared pool, return control to the shared lifecycle; verify via existing operator-plane contract (admin router answers, readiness gates on database probe)
- [ ] 6.2 RED: add `services/dispatcher/tests/end_to_end.rs` `operation_lifecycle_renders_progress_then_terminal_once_through_fake_bot_api`: seed owner identity/chat + a send-job-created binding against a spawned fake Bot API server (bot-api harness pattern), deliver accepted→running→running→succeeded snapshots with duplicate envelope ids and one stale event through the seam, assert the server saw ordered throttled edits, exactly one terminal render, and final states `sent` with correct binding revision
- [ ] 6.3 GREEN: close any wiring gaps until 6.2 passes; keep the dispatcher main a role constant plus one harness call
- [ ] 6.4 Verify restart recovery end to end: kill-and-rerun variant of 6.2 where the process stops after enqueue but before delivery; the restarted run delivers the orphaned job exactly once (asserts spec "survives a restart")

## 7. Change verification

- [ ] 7.1 Run the full DEVELOPMENT.md gate command list (with `TELEGRAM_TEST_DATABASE_URL` pointing at the local instance) plus `openspec validate --strict`; all come back clean
- [ ] 7.2 Inspect `git diff` for leaked secrets, tokens, or real chat/user identifiers; confirm fixtures use synthetic values and no message content lands in logs or metric labels
- [ ] 7.3 Update README status paragraph, DEVELOPMENT.md stage/commands (dispatcher now requires database; new configuration keys), and note the item as implemented in docs/IMPLEMENTATION_PLAN.md order; archive the change only after 7.1 is green and every task above is ticked
