## 1. Schema and persistence bindings

- [x] 1.1 RED: add `crates/persistence/tests/bindings.rs` with catalog test `identities_and_chats_exist_with_expected_shape` asserting `telegram.identities` (PK `telegram_user_id`, nullable `internal_user_id`, display snapshot columns, access-state CHECK, timestamps) and `telegram.chats` (PK `chat_id`, type CHECK admitting `'private'`, access-state CHECK, timestamps) exist; confirm the test fails on missing relations before touching `schema.sql`
- [x] 1.2 GREEN: declare both tables in root `schema.sql`; rerun until 1.1 passes
- [x] 1.3 RED: add repository tests in `crates/persistence/tests/bindings.rs` for `ensure_identity`/`find_identity` and `ensure_chat`/`find_chat` asserting insert-if-absent returns the existing row unchanged and never flips access state
- [x] 1.4 GREEN: implement the bindings repository in `crates/persistence/src/bindings.rs`
- [x] 1.5 RED: add `settle_denied_marks_terminal_and_minimizes_payload` in `crates/persistence/tests/updates.rs` mirroring the unsupported-settlement test: state becomes `denied`, the payload is removed, dedupe evidence is retained
- [x] 1.6 GREEN: add the `denied` transition beside the other terminals in `crates/persistence/src/updates.rs`

## 2. Access configuration

- [x] 2.1 RED: add parse tests in `crates/core/src/config` named `access_owner_telegram_user_id_parses_positive_i64` and `access_owner_telegram_user_id_refuses_zero_negative_non_integer` asserting the V14 violation text for bad values
- [x] 2.2 GREEN: add the ACCESS table struct and field parsing in `crates/core/src/config/model.rs`
- [x] 2.3 RED: add validation-rule tests in `crates/core/src/config/validate.rs`: the webhook role without the key yields a violation naming it; the dispatcher with default configuration validates cleanly
- [x] 2.4 GREEN: implement rule V14 in `crates/core/src/config/validate.rs`
- [x] 2.5 Document `RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID` in `.env.example` including the deploy-with-binary caveat and the direct-SQL disable note (no failing test possible: static example file)

## 3. Authorization gate and bootstrap in the webhook

- [x] 3.1 RED: add `unauthorized_sender_settles_denied_without_outbound_calls` in `services/webhook/tests/intake.rs`: deliver an update from an unenrolled user and assert the update row settles `denied` with a minimized payload while the fake Bot API records no requests
- [x] 3.2 GREEN: implement the gate in `services/webhook/src/intake/worker.rs`: resolve sender and chat, evaluate the policy, settle `denied` before any domain action
- [x] 3.3 RED: extend the policy matrix in `services/webhook/tests/intake.rs`: a disabled identity and a group-chat delivery deny identically to the unknown sender, and groups gain no chat row
- [x] 3.4 GREEN: complete the policy evaluation so the whole matrix passes
- [x] 3.5 RED: add `startup_provisions_owner_once_without_resurrection` in `services/webhook/tests/boot.rs`: a fresh database ends with exactly one enabled owner row, and a pre-disabled row stays disabled and singular after restart
- [x] 3.6 GREEN: wire `ensure_owner` into webhook startup using the configured owner id

## 4. Change verification

- [x] 4.1 Run the full DEVELOPMENT.md gate command list plus `openspec validate --strict`; both come back clean
- [x] 4.2 Inspect `git diff` for leaked secrets, tokens, or real chat/user identifiers; confirm fixtures use synthetic values
