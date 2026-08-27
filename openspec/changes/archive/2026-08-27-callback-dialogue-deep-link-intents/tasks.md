## 1. Generalized current-schema foundation

- [x] 1.1 RED: add `crates/persistence/tests/interaction_state_schema.rs::fresh_schema_contains_generalized_dialogue_and_token_authority`; assert `telegram.dialog_states` and `telegram.interaction_tokens` plus their scope/version/expiry/consumption constraints exist, run it through `build-gate -- cargo nextest run --locked -p telegram-persistence --test interaction_state_schema`, and confirm it fails because both tables are absent (not because it does not compile)
- [x] 1.2 GREEN: add the two generalized tables and indexes to root `schema.sql` alongside the still-needed legacy tables, rerun 1.1 until green, then run the persistence crate tests through the build gate
- [x] 1.3 RED: add `crates/persistence/src/interaction_tokens.rs::tests::minted_token_uses_the_full_url_safe_callback_budget`; assert the existing minted value is exactly 64 URL-safe ASCII bytes, run the exact unit test through the build gate, and confirm it fails with the current 43-byte token
- [x] 1.4 GREEN: enable UUID v4 on the existing pinned dependency, mint three independent UUIDv4 byte arrays into one unpadded URL-safe token, rerun 1.3 until green, and run the persistence crate tests through the build gate

## 2. Scoped single-use token registry

- [x] 2.1 RED: add `crates/persistence/tests/interaction_tokens.rs::token_expires_at_its_boundary`; add only the minimal nonfunctional registry signature needed for the test to compile, assert presentation at `expires_at` returns `TokenRefusal::Expired` without consumption, run the exact test through the build gate, and confirm the assertion receives the stub refusal instead
- [x] 2.2 GREEN: implement typed token creation/locking and expiry classification in `crates/persistence/src/interaction_tokens.rs`, rerun 2.1 until green, and run the persistence crate tests through the build gate
- [x] 2.3 RED: add `crates/persistence/tests/interaction_tokens.rs::scope_mismatch_preserves_the_owners_live_token`; assert wrong bot/user/chat/message presentations return the safe scope refusal and the correct owner can still consume afterward, run the exact test through the build gate, and confirm the current implementation accepts or invalidates at least one mismatched presentation
- [x] 2.4 GREEN: enforce the complete scope before mutation, rerun 2.3 until green, and run the persistence crate tests through the build gate
- [x] 2.5 RED: add `crates/persistence/tests/interaction_tokens.rs::concurrent_single_use_presentations_have_one_winner`; assert two concurrent consumes return one action and one `Consumed` refusal and the row records one consumer, run the exact test through the build gate, and confirm more or fewer than one action is released
- [x] 2.6 GREEN: make one transaction lock, validate, consume, and commit the token authority, rerun 2.5 until green, and run the persistence crate tests through the build gate

## 3. Durable dialogue transitions

- [x] 3.1 RED: add `crates/persistence/tests/dialogues.rs::awaiting_dialogue_survives_a_new_database_handle`; add only minimal compiling dialogue repository signatures, assert a re-read preserves owner scope, step, version, safe payload, and expiry, run the exact test through the build gate, and confirm the stub returns no dialogue
- [x] 3.2 GREEN: implement the closed dialogue types and create/read repository methods in `crates/persistence/src/dialogues.rs`, rerun 3.1 until green, and run the persistence crate tests through the build gate
- [x] 3.3 RED: add `crates/persistence/tests/dialogues.rs::only_one_expected_version_transition_wins`; assert two concurrent transitions from the same step/version produce one incremented state and one stale-state refusal, run the exact test through the build gate, and confirm the current transition path admits the wrong winner count
- [x] 3.4 GREEN: implement the scoped expected-step/version compare-and-swap transition in one transaction, rerun 3.3 until green, and run the persistence crate tests through the build gate
- [x] 3.5 RED: add `crates/persistence/tests/dialogues.rs::awaiting_input_expires_at_the_timeout_boundary`; assert a transition at expiry returns `Expired`, persists terminal `expired`, increments the version exactly once, and releases no action, run the exact test through the build gate, and confirm the dialogue remains active or returns the stub refusal
- [x] 3.6 GREEN: implement timeout-to-expired as an atomic terminal transition, rerun 3.5 until green, and run the persistence crate tests through the build gate

## 4. Deep-link start intents

- [x] 4.1 RED: add `services/webhook/src/intake/intent.rs::tests::start_command_parses_only_a_64_character_opaque_token`; assert exact `/start <token>` returns a token value while URL/JSON/provider-id/short/extra-argument payloads do not, run the exact unit test through the build gate, and confirm the valid form is currently rejected
- [x] 4.2 GREEN: add the closed start-token grammar before URL capture parsing, rerun 4.1 until green, and run webhook unit tests through the build gate
- [x] 4.3 RED: add `crates/persistence/tests/interaction_tokens.rs::deep_link_intent_resolves_once_for_its_bound_owner`; assert an operation-status deep-link token releases its typed server-side source/blob payload once, a replay releases nothing, and the Platform operation row/reference is unchanged, run the exact test through the build gate, and confirm the registry does not yet resolve the typed intent
- [x] 4.4 GREEN: implement deep-link issue, live-by-operation lookup, and owner-scoped one-time consume on the generalized registry; migrate capture and dispatcher persistence calls, rerun 4.3 plus the capture/dispatcher suites until green
- [x] 4.5 RED: update `services/dispatcher/tests/payload.rs::attachment_success_uses_blob_facts_and_opaque_deep_link` to require `https://t.me/<bot>?start=<64-char-token>` and no business data, run that exact test through the build gate, and confirm it fails because the renderer still emits `startapp=<uuid>`
- [x] 4.6 GREEN: render the generalized operation token in the `start` query parameter, rerun 4.5 and dispatcher tests until green
- [x] 4.7 RED: add `services/webhook/tests/deep_links.rs::valid_start_token_is_consumed_but_replay_and_foreign_scope_release_nothing`; deliver synthetic owner, replay, and foreign `/start` updates, assert only the first owner delivery resolves the intent and all domain operation/message-binding state remains unchanged, run the test through the build gate, and confirm the worker currently treats every delivery as unsupported
- [x] 4.8 GREEN: route valid start-token messages through scoped registry consumption without inventing a new domain command, rerun 4.7 and webhook tests until green

## 5. Reconcile the GitHub callback flow

- [x] 5.1 RED: add `services/webhook/tests/github_flow.rs::second_press_is_answered_as_expired_without_another_action`; press one recognized selection token twice, assert two callback answers, one transition, zero GitHub action calls, and the second durable reply is exactly `This action has expired. Please start again.`, run it through the build gate, and confirm the existing generic unavailable message fails the assertion
- [x] 5.2 GREEN: model repository preview/selection/confirm/cancel as the `github_repository` dialogue kind and issue every button through the generalized registry; preserve expected-message stamping and the stable action identity, rerun 5.1 and the complete GitHub flow suite until green
- [x] 5.3 REFACTOR: run the existing persistence and webhook GitHub tests green before and after replacing `callback_flows` call sites/tests with the dialogue/token repositories; add no behavior or test in this task
- [x] 5.4 RED: add `crates/persistence/tests/interaction_state_schema.rs::legacy_item_specific_interaction_tables_are_absent`; assert `callback_flows`, `callback_tokens`, and `interaction_intents` are absent, run it through the build gate, and confirm all three still exist
- [x] 5.5 GREEN: remove the three superseded tables/modules and every legacy reference after `rg` confirms no caller remains, rerun 5.4 plus persistence/webhook/dispatcher tests until green

## 6. Bounded cleanup lifecycle

- [x] 6.1 RED: add `crates/persistence/tests/interaction_cleanup.rs::cleanup_expires_dialogues_and_removes_only_eligible_tokens_in_one_bounded_batch`; assert the exact stale rows change, live authority remains consumable, the reported counts do not exceed the batch bound, and operation/message-binding rows are unchanged, run it through the build gate, and confirm the minimal compiling cleanup stub changes nothing
- [x] 6.2 GREEN: implement stable-order bounded cleanup for dialogue expiry, stale token deletion, and retention-expired terminal dialogue deletion in one owned transaction, rerun 6.1 until green, and run the persistence crate tests through the build gate
- [x] 6.3 RED: add `services/webhook/tests/interaction_cleanup.rs::worker_runs_cleanup_on_startup`; close an otherwise idle worker after startup and assert a pre-existing stale dialogue reached `expired`, run it through the build gate, and confirm the current worker exits without cleanup
- [x] 6.4 GREEN: run cleanup before the worker claim loop and thereafter on a fixed monotonic interval without a second detached task, rerun 6.3 and webhook tests until green

## 7. Documentation, validation, and delivery readiness

- [x] 7.1 Update `README.md`, `DEVELOPMENT.md`, `docs/ARCHITECTURE.md`, `docs/DATA_MODEL.md`, `docs/INTERFACES.md`, `docs/TESTING.md`, and `docs/IMPLEMENTATION_PLAN.md` for the delivered token/dialogue/start/cleanup behavior; no RED applies because these are documentation artifacts, then verify terminology and the item-8 status with `rg`
- [x] 7.2 Add bounded safe telemetry for token outcomes, dialogue transitions/timeouts, and cleanup counts without token/user/chat/payload labels; no RED applies to static metric declarations, then run telemetry tests and repository lint through the build gate
- [x] 7.3 Run `cargo fmt --all -- --check`, `git diff --check`, `openspec validate callback-dialogue-deep-link-intents --strict`, every command in the fenced gate block of `DEVELOPMENT.md` through the required machine-wide build gate, and the complete relevant PostgreSQL integration suites against a verified reachable disposable endpoint; record exact outcomes before marking complete
- [x] 7.4 Review the final diff and repository-wide searches for legacy table/API names, `startapp` operation links, leaked token values, migrations, compatibility paths, skipped/focused tests, TODOs, and unrelated edits; this is verification-only and must be clean before archive
