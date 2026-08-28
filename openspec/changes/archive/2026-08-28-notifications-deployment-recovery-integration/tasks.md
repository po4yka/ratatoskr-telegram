## 1. Preconditions and dependency surface

- [x] 1.1 Verify the exact Contracts revision contains `ratatoskr-event-envelope` and `ratatoskr-notification-contracts`, and verify the green Platform `provision-telegram-notification-consumer` commit is recorded in workspace `TG-010`; configuration/dependency ordering cannot start from a failing behavior test, so record `git cat-file` and changeset evidence before editing production code.
- [x] 1.2 Add pinned contract crates and fleet-aligned `async-nats = 0.50.0` to the narrow dispatcher boundary, update the lockfile through `build-gate`, and verify `cargo metadata --locked` has one NATS client line, exact Contracts revisions, target-compatible features, and no new native/provider dependency.

## 2. Current schema and explicit chat authority

- [x] 2.1 Add `notification_schema_enforces_bindings_preferences_and_decision_uniqueness` to `crates/persistence/tests/schema.rs`, create a fresh database, and verify it fails because the binding/preference/decision tables and constraints are absent.
- [x] 2.2 Edit `schema.sql` in place to add explicit private-chat bindings, normalized policies/class overrides, notification decisions/transport failures, and outbound delivery metadata; verify the schema test passes with invalid quiet windows/unbound policies rejected and concurrent `(notification_id, chat_id)` inserts deduplicated.
- [x] 2.3 Add `admitted_private_chat_creates_explicit_actor_binding` and `similar_numeric_chat_is_not_authorized` to `services/webhook/tests/access.rs`; verify the first fails because admission stores no relation and the second proves no ID-equality fallback.
- [x] 2.4 Persist the explicit binding only after all existing actor/chat/access checks pass and expose bounded lookup APIs; verify both access tests pass and denied/group updates create no binding.
- [x] 2.5 Add `preference_write_is_versioned_and_atomic` to `crates/persistence/tests/notification_preferences.rs`, covering defaults, one class override, malformed windows, foreign binding, and stale version; verify it fails because no preference repository exists.
- [x] 2.6 Implement typed preference reads/updates and database invariants with injected timestamps; verify the preference test passes and direct SQL cannot bypass the same quiet-hours/binding constraints.

## 3. Deterministic settings interaction

- [x] 3.1 Add `settings_commands_inspect_and_update_notification_policy` to `services/webhook/tests/settings.rs`, drive every documented exact form plus malformed/foreign cases, and verify it fails because `/settings` is not routed to notification policy.
- [x] 3.2 Implement the closed `/settings` parser/renderer and optimistic preference transitions, reusing the existing authorization/escaping/outbound queue; verify the settings test passes with sibling overrides unchanged and no partial malformed write.

## 4. Typed notification consumer and policy decisions

- [x] 4.1 Add notification-bus cases to `crates/core/tests/notification_config.rs` for the exact stream/durable/subject, positive bounds, local-or-TLS endpoint, readable seed file, mutually exclusive secret sources, wildcard refusal, and redacted errors; verify canonical configuration currently fails as unknown.
- [x] 4.2 Add finite secret-file and notification-bus types/resolution/validation in `crates/core/src/config/`, with centralized reads and no value rendering; verify all new and existing role configuration tests pass.
- [x] 4.3 Add `dispatcher_requires_matching_preprovisioned_notification_durable` to `services/dispatcher/tests/notifications.rs`, run against real JetStream with missing, foreign-filtered, and matching consumers, and verify the current dispatcher incorrectly has no notification dependency/readiness behavior.
- [x] 4.4 Add the dispatcher NKey connection, exact durable verification, cancel-safe consumer lifecycle, and readiness component without consumer-create authority; verify the durable/readiness test passes and shutdown drains within the configured ceiling.
- [x] 4.5 Add `notification_admission_enforces_policy_and_quiet_hours` to `services/dispatcher/tests/notifications.rs`, covering global/class suppression, custom/inherited/disabled hints, normal/wrapping boundaries, high-priority bypass, unknown class, no eligible chat, and injected UTC time; verify it fails with no decisions/jobs.
- [x] 4.6 Implement pure quiet-hours evaluation, explicit recipient resolution, privacy-minimal rendering, and one transactional inbox/decision/outbound admission; verify the policy test passes and its snapshots contain no raw envelope or unauthorized chat.
- [x] 4.7 Add `notification_identity_deduplicates_distinct_event_envelopes` and `concurrent_notification_admission_creates_one_job` to the dispatcher notification test; verify both fail because transport event IDs currently provide the only dedupe key.
- [x] 4.8 Enforce `(notification_id, chat_id)` insertion-as-decision and acknowledge only after commit; verify duplicate/redelivery/concurrency tests pass with one job, both bus deliveries settled, and database failure negatively acknowledged.
- [x] 4.9 Add malformed-event cases to `services/dispatcher/tests/notifications.rs` for wrong type, invalid contract, missing recipient binding, and content-bearing input; verify they fail because bounded transport failure evidence and poison-message settlement do not exist.
- [x] 4.10 Record only stream sequence/event ID when available and a closed failure class, terminally settle permanent invalid input, and retry transient database/bus failures within bounds; verify malformed cases pass with no payload/body/title/user/chat data persisted or logged.

## 5. Outbound priority, deferral, and settlement

- [x] 5.1 Add `direct_jobs_precede_new_notifications_and_old_notifications_age_in` to `crates/persistence/tests/outbound_jobs.rs`, preserving one in-flight job and FIFO within class; verify it fails because claims currently use one undifferentiated FIFO.
- [x] 5.2 Add delivery class, notification linkage, and bounded due-time aging to enqueue/claim APIs and indexes; verify the priority test passes without changing projection superseding or rate limits.
- [x] 5.3 Add `deferred_notification_becomes_claimable_once_at_release` to the outbound persistence tests, wake the claim loop repeatedly before/at/after the boundary, and verify it fails because no decision-linked deferred job exists.
- [x] 5.4 Insert one future-due job during notification admission and settle the linked decision from sender outcomes; verify boundary and permanent blocked/forbidden-chat tests pass with no duplicate job or unbounded retry.
- [x] 5.5 Add `notification_renderer_escapes_dynamic_markup_and_omits_private_fields` to dispatcher tests and verify it fails against the absent renderer.
- [x] 5.6 Implement a bounded HTML/plain-text notification renderer with only title, optional detail, and authorized opaque links; verify metacharacter and privacy tests pass for every contract class plus an unknown class.

## 6. Readiness and content-free telemetry

- [x] 6.1 Extend `crates/http/tests/admin.rs` and dispatcher integration tests with `notification_dependency_controls_dispatcher_readiness`, asserting safe database/credential/NATS/durable/consumer-loop classes; verify it fails because notification consumption is not represented.
- [x] 6.2 Wire notification dependency state into the existing operator plane and startup/shutdown lifecycle; verify readiness stays false for missing/mismatched durable and contains no endpoint, credential, user, chat, or payload.
- [x] 6.3 Add telemetry tests for received/duplicate/enabled/suppressed/deferred/enqueued/delivered/retry/terminal outcomes, lag, and backlog; include an unknown class and verify they fail because the metric family does not exist.
- [x] 6.4 Add closed-label notification instruments and safe structured events, mapping unknown classes to `other`; verify telemetry tests pass and secret/content/identifier scans find no dynamic metric label or raw error.

## 7. Single-host deployment artifacts

- [x] 7.1 Add `services/webhook/tests/deployment_profile.rs` (shared with dispatcher as appropriate) with `systemd_profile_matches_runtime_and_workspace_contract`, asserting two role units, exact `8182`/`9467`/`9468` listeners, no dispatcher public bind, `Type=exec`, `TimeoutStopSec=130s`, role ordering, resource/hardening directives, NVMe logging/rotation, and secret-file-only examples; verify it fails because `deploy/` is absent.
- [x] 7.2 Add webhook/dispatcher units, environment examples, install/check instructions, monitoring/firewall snippets, and logrotate policy under `deploy/`; verify structural tests pass and no checked-in artifact contains a credential value or target mutation command executed by validation.
- [x] 7.3 Add `deployment_profile_refuses_role_or_schema_drift` to the structural tests, mutate copied fixtures to wrong ports, timeout, missing resource limit, and dispatcher-before-schema ordering, and verify each fixture is rejected.
- [x] 7.4 Add finite `check-config`/schema-check process modes and align unit `ExecStartPre`/ordering with the runtime constants; verify drift cases and fresh-database boot tests pass while webhook is the sole schema applier.

## 8. Executable recovery and rotation runbooks

- [x] 8.1 Add `deploy/tests/telegram_ops_test.sh` with `rotation_dry_runs_are_redacted_and_non_mutating`, using synthetic candidate files and fake `systemctl`/`curl`; verify it fails because the guarded tool and runbooks are absent.
- [x] 8.2 Add `deploy/bin/telegram-ops` plus webhook-secret and bot-token rotation runbooks with candidate validation, atomic local replacement/rollback, affected-role readiness checks, and separately authorized provider writes; verify dry runs pass without file/process/network changes or secret output.
- [x] 8.3 Extend the ops test with `session_inspection_uses_platform_authority` and `stuck_recovery_requires_expected_state_and_execute`, using synthetic adapters; verify it fails because no allowlisted session/recovery commands exist.
- [x] 8.4 Add session/stuck-operation runbooks and parameterized Telegram-owned inspection/recovery SQL, keeping Platform session/status mutations behind its authorized surface; verify read-only default, stale-state refusal, exact one-row conditional recovery, and no cross-schema write.
- [x] 8.5 Extend the ops test with `dead_inspection_is_bounded_and_read_only`, populate synthetic failed updates/outbound/notification rows with private payloads, and verify it fails because no safe projection command exists.
- [x] 8.6 Add dead-update/dead-outbound/dead-notification inspection runbooks and projections limited to identifiers, timestamps, attempts, safe class, and correlation reference; verify output omits payload, text, title, username, chat ID, credential, and provider diagnostics.
- [x] 8.7 Add `runbook_commands_execute_as_written` to extract or invoke every documented command in dry-run/read-only mode with fake host tooling; verify it fails for any stale path, option, syntax, mutation, or undeclared external-authority boundary.
- [x] 8.8 Make runbooks call only the tested tool interface and complete its help/error contract; verify the complete shell suite passes `bash -n`, ShellCheck if already present in the gate, and the deterministic dry-run fixtures.

## 9. Repository and workspace verification

- [x] 9.1 Update README/deployment/config/notification documentation and `DEVELOPMENT.md`/CI gate lists together; documentation cannot start from a failing behavior test, so verify all paths/commands through structural/runbook tests and the gate-list drift check.
- [x] 9.2 Run the targeted failing-then-passing test history, `cargo fmt --all -- --check`, affected tests, full `cargo test --workspace --locked`, clippy/security/license checks, and every fenced command in `DEVELOPMENT.md` through `build-gate` where compiler-backed; record exact outcomes without claiming hosted CI or live deployment.
- [x] 9.3 Run workspace TG-010 against the exact Platform and Telegram commits and verify plan-item-5 article completion, enabled notification delivery, disabled suppression, duplicate dedupe, and task-namespaced cleanup; record this as synthetic composed evidence, not live Telegram/provider proof.
- [x] 9.4 Run `openspec validate notifications-deployment-recovery-integration --strict`, review the final diff for schema-version/migration violations, unsafe recovery, secret leakage, unrelated changes, and stale call sites, then mark tasks complete only after their named evidence was observed.
