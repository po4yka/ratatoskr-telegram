## 1. Contract Pin and Platform Client

- [x] 1.1 Pin the merged GitHub-interaction contracts SHA and update the lockfile; this dependency step cannot start from a failing behavior test, so verify `cargo metadata --locked --no-deps` resolves the immutable revision and no local path override exists
- [x] 1.2 Add `crates/platform-api/tests/github.rs::preview_and_action_calls_use_platform_session_and_shared_contract`; predict and observe failure because the client has no `/v1/gh/repositories` methods
- [x] 1.3 Implement typed preview/action client methods with bounded transient classification and same-key retry inputs; verify the named test and all `platform-api` tests pass

## 2. GitHub URL Routing

- [x] 2.1 Add `services/webhook/src/intake/github.rs::tests::only_exact_repository_urls_route_to_preview`; predict and observe failure because generic article parsing still claims canonical GitHub repository URLs
- [x] 2.2 Implement the pure canonical repository parser and route it before generic bare-URL capture while keeping explicit `/summarize` and forwarded-link behavior; verify the named test plus article-capture tests pass

## 3. Preview Rendering

- [x] 3.1 Add the preview assertions in `services/webhook/tests/github_flow.rs::repository_preview_confirmation_gate_and_partial_result_are_truthful`; observe the flow fail before the GitHub worker/card/token path is complete
- [x] 3.2 Implement authenticated preview submission, escaped optional-field rendering, capability-filtered opaque selection buttons, and durable outbound enqueueing; verify the named test records one preview, zero actions, and the exact safe card

## 4. Callback Flow Persistence

- [x] 4.1 Add `crates/persistence/tests/callback_flows.rs::schema_and_store_enforce_owner_message_expiry_version_and_one_winner`; predict and observe failure because callback flow/token tables and APIs are absent
- [x] 4.2 Edit `schema.sql` in place and implement flow/token creation, provider-message stamping after successful send, transactional consumption, expiry, owner/chat/message/stage/version checks, and stable idempotency identity; verify the named test and all schema/persistence tests pass

## 5. Selection and Confirmation Gate

- [x] 5.1 Add the zero-action selection gate to `repository_preview_confirmation_gate_and_partial_result_are_truthful`; verify the harness observes no action before confirmation
- [x] 5.2 Implement owner-bound selection consumption, prompt composition, distinct confirm/cancel token minting, prompt flow binding, and prompt callback acknowledgment; verify selection produces zero action calls
- [x] 5.3 Add persistence coverage for foreign, expired, concurrent, and replayed token presentations plus the confirmed-action assertion in the end-to-end harness
- [x] 5.4 Implement confirm/cancel transitions, prompt callback acknowledgment, durable `submitting` state, same-idempotency recovery, and safe stale/foreign replies; verify one concurrent confirm wins and every non-confirm path records zero action calls

## 6. Truthful Result Projection

- [x] 6.1 Add component-level partial-result assertions to `repository_preview_confirmation_gate_and_partial_result_are_truthful` and pure rendering table tests
- [x] 6.2 Implement exhaustive metadata/provider-star/desired-backup rendering, optional-field omission, result persistence before enqueue, and no compensation control; verify the named partial result and refused/already/accepted/skipped table cases pass
- [x] 6.3 Add `services/webhook/tests/github_flow.rs::uncertain_action_retries_only_the_same_identity_and_never_claims_success`
- [x] 6.4 Implement bounded same-key retry/recovery and safe unavailable/unknown messages for preview/action failures; verify the harness sees no second action identity and no fabricated success

## 7. Live Dependency Gate

- [x] 7.1 Verify the prerequisite live `ratatoskr-github` repository API and shared contract at merged main `fd60dd37e22b30056d2153ef22b271f75659e654`; the producer live smoke exercised preview plus truthful partial action before Telegram implementation began
- [x] 7.2 Wire the Telegram fake Platform/GitHub and Bot API harness; verify the test exercises repository URL -> preview -> selection -> confirmation -> truthful partial result, with zero pre-confirmation writes

## 8. Documentation, Gate, and Delivery

- [x] 8.1 Update README plan status, commands/buttons, callback retention, `docs/INTERFACES.md`, architecture/security/telemetry notes, and current schema docs; documentation cannot start from a failing behavior test, so verify it names only implemented surfaces and keeps OAuth/list UI out of scope
- [x] 8.2 Run the exact fenced gate from `DEVELOPMENT.md` through `build-gate`, run `openspec validate github-repository-preview-confirmation-flow --type change --strict`, inspect `git diff --check` and the complete diff, and rerun the live GitHub dependency test
- [ ] 8.3 Fetch/rebase on current `origin/main`, rerun targeted/full/live gates, commit only this change, integrate it into Telegram `main`, push `main`, then remove the merged task worktree and delete the fully merged task branch with `git branch -d`
