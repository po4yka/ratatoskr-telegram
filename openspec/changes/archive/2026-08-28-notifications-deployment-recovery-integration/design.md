## Context

See `proposal.md` for motivation. The repository already has a fast webhook, a durable PostgreSQL outbound queue, a dispatcher with per-chat ordering/rate limiting, explicit Telegram identity and private-chat access records, Platform operation projection, typed configuration, and private operator endpoints. It has no explicit identity-to-private-chat relation, notification policy, notification bus client, or production deployment/recovery assets.

The workspace change `telegram-notification-deployment-integration` fixes the cross-repository route and port allocation. Platform change `provision-telegram-notification-consumer` owns creation of `ratatoskr_telegram_notifications` and the least-privilege NATS identity. This repository must open and verify that durable but cannot create or alter it. Development status requires editing `schema.sql` in place and forbids migrations or parallel schema/API versions.

## Goals / Non-Goals

**Goals:**

- Make notification admission, preference evaluation, deferral, and outbound enqueue one replay-safe database decision.
- Keep recipient selection explicit and private-chat-only.
- Reuse the existing outbound reliability boundary while preventing background notification starvation or interference with direct interactions.
- Ship production-shaped, structurally testable units and executable, fail-closed recovery tooling.

**Non-Goals:**

- Channel or scheduled digest generation, a generic automation engine, or producer scheduling.
- Domain notification creation, provider API calls other than Telegram Bot API, or Platform session-table writes.
- A user-facing operator/admin health command or LLM model controls.
- Live credential rotation, webhook registration, host installation, or real-chat proof.

## Decisions

### 1. Use the fleet-standard NATS client and pinned contract crates

Add `async-nats` at the same `0.50.0` line used by Platform plus `ratatoskr-event-envelope` and `ratatoskr-notification-contracts` at the repository's existing exact Contracts revision. The dispatcher connects with a user NKey seed read from a file, opens the existing `ratatoskr_events` stream and `ratatoskr_telegram_notifications` pull consumer, and verifies durable name, exact subject filter, pull mode, and explicit acknowledgements before readiness becomes true.

This adds no new provider SDK and no native system dependency: all three dependencies are already fleet-standard, BSD/Apache-compatible Rust crates in the current lock/security posture and support the target `aarch64-unknown-linux-gnu`. A custom NATS protocol client was rejected as unsafe and unnecessary. Broad `$JS.API.>` authority or consumer auto-creation was rejected because Platform owns bus topology.

### 2. Add an explicit private-chat binding and normalized preference tables

Extend current `schema.sql` with:

- `private_chat_bindings`: one admitted Telegram identity for one private chat, with foreign keys to Telegram-owned identity/chat rows and timestamps; the chat key is unique so numeric resemblance never creates authority;
- `notification_preferences`: one row per bound internal user/chat, global enabled, quiet mode, optional UTC minute bounds, high-priority bypass, optimistic version, and audit timestamps;
- `notification_class_preferences`: zero or one override per known class and policy, where absence means inherit global;
- `notification_decisions`: one row per `(notification_id, chat_id)` with recipient reference, source event, preserved class token, decision state, release time, linked outbound job, and closed safe failure class;
- `notification_transport_failures`: one bounded diagnostic row per failed JetStream stream sequence, with no payload body.

The access gate writes `private_chat_bindings` only after the actor, owner allowlist, private chat, and active access rows all pass. `/settings` creates the default policy lazily as global enabled, no class overrides, quiet mode `inherit`, and high-priority bypass disabled. The six contract classes in the pinned crate are the closed settings choices: `operation_completed`, `operation_failed`, `analysis_ready`, `backup_outcome`, `watch_triggered`, and `archive_imported`. Unknown future class tokens remain valid delivery inputs but cannot be selected as an override in this build.

Embedding a JSON policy blob was rejected because database constraints, per-class atomic updates, and operator inspection would become weaker. Using `chat_id == telegram_user_id` was rejected because Telegram identifiers are not authorization relationships.

### 3. Evaluate preferences and enqueue in one transaction

For each valid envelope, one persistence operation:

1. inserts the envelope event ID into `telegram.inbox`;
2. resolves enabled internal-user identity rows through explicit enabled private-chat bindings;
3. locks or lazily creates each chat policy;
4. evaluates global/class settings and quiet hours against an injected UTC clock;
5. inserts the deduplicated decision and, for enabled/deferred outcomes, one linked outbound job;
6. commits before the JetStream message is acknowledged.

The `(notification_id, chat_id)` key, not `event_id`, prevents a republished notification from creating another message. A duplicate event or notification commits no second job and is acknowledged. A database/transient failure is negatively acknowledged with bounded delay. Invalid contract/binding input is reduced to a closed failure class, recorded without content, and terminally acknowledged so poison input cannot loop forever.

Doing deduplication in memory or acknowledging before commit was rejected because either loses restart safety. Storing the full event was rejected because the contract fields needed for a decision are already projected into bounded columns and the payload can contain private text.

### 4. Quiet hours are pure UTC minute arithmetic

Represent daily boundaries as minutes `0..1439`. Equal bounds are invalid. A non-wrapping window is active at `start <= now < end`; a wrapping window is active at `now >= start || now < end`. The next release is the first matching end strictly after the admission instant. All calculations use an injected clock and whole UTC minutes; no host timezone or daylight-saving rule participates.

Custom policy overrides the producer hint, `disabled` ignores it, and `inherit` validates and uses it. High-priority bypass occurs only when both the contract priority is high and the user's own flag is true. Deferred delivery inserts the one outbound job immediately with `next_attempt_at` equal to the release instant, so repeated polling never creates another job.

A timezone database and per-user locale were rejected because the contract and accepted scope provide UTC offsets only. A scheduler subsystem was rejected because the existing durable queue already has time eligibility.

### 5. Add a notification delivery class with bounded aging

Add `delivery_class` (`direct`, `projection`, `notification`) and optional `notification_id` to outbound jobs. Claiming remains one in-flight job per chat and FIFO within a class. A ready direct/projection job precedes a newly due notification; a notification that has been due for the configured `background_max_wait_seconds` is promoted ahead of newer direct work. This gives interactive work immediate priority without indefinite notification starvation. The promotion clock starts at `next_attempt_at`, so quiet-hour deferral is not counted as queue starvation.

Separate notification workers or a second queue were rejected because they would race the existing per-chat sender and rate limiter. Permanent Bot API failure settles both the outbound job and linked notification decision with the existing closed provider class.

### 6. Keep `/settings` deterministic and health private

The webhook command parser accepts only the exact forms in the notification spec. Every mutation re-runs access authorization, checks the explicit chat binding, and updates the expected preference version transactionally. `/settings` renders the global value, each known override/inherit value, UTC quiet mode/window, and bypass choice with escaped text; malformed values change nothing and return bounded usage help.

No `/admin`, `/dbinfo`, `/dbverify`, or process-health Telegram command is added. Existing `/status <operation>` remains an authorized user operation projection. `/health/live`, `/health/ready`, `/metrics`, and `/version` remain on the operator listener; dispatcher readiness adds database, credential-file, NATS, durable compatibility, and consumer-loop components.

### 7. Secret-file configuration is explicit and mutually exclusive

Each existing sensitive value gains an optional file source and one resolver that returns a secret only after validation: bot token, webhook secret, Platform assertion signing key, and NATS seed. Inline values remain available for bounded local/test use, but configuring inline and file sources together fails before any listener binds. Effective configuration and errors expose only the setting name, path class, and safe reason.

Add a finite `notification_bus` section with local/HTTPS-safe URL rules, fixed stream/durable/subject, positive fetch/ack/poll bounds, seed path, and notification aging bound. Unknown fields and wildcard subjects fail closed. Environment reads remain centralized in the existing Figment loader.

### 8. Deploy webhook and dispatcher as two hardened systemd roles

Ship `ratatoskr-telegram-webhook.service` and `ratatoskr-telegram-dispatcher.service` plus non-secret environment examples and logrotate policy. The webhook binds `127.0.0.1:8182` for cloudflared and `0.0.0.0:9467` for bounded monitoring; dispatcher binds only `0.0.0.0:9468`. Webhook is the sole current-schema applier. Dispatcher orders after/requires webhook and runs a schema/config compatibility check before execution.

Both units use `Type=exec`, `TimeoutStopSec=130s`, `Restart=always`, bounded delay without a startup latch, role-specific Unix users/groups, `NoNewPrivileges`, `PrivateTmp`, `ProtectSystem=strict`, limited writable paths, syscall/capability restrictions compatible with Rust networking, `TasksMax=128`, and explicit CPU/memory ceilings. The webhook ceiling is `MemoryHigh=384M`, `MemoryMax=512M`, `CPUQuota=100%`; dispatcher is `MemoryHigh=256M`, `MemoryMax=384M`, `CPUQuota=100%`. Logs append under `/mnt/nvme/ratatoskr/logs` and rotate there. Credential values live only in root-owned files named by configuration.

One combined unit was rejected because webhook admission and asynchronous dispatch have different exposure, dependency, and failure boundaries. `Type=notify` was rejected because neither binary calls `sd_notify`.

### 9. Recovery is an allowlisted script plus parameterized SQL

Ship `deploy/bin/telegram-ops` with explicit subcommands and safe defaults: rotation planning/validation, session inspection, stuck-work inspection, conditional recovery, and dead-update/outbound/notification inspection. Read-only and dry-run are the defaults. Mutations require `--execute`, exact UUID/integer/state validation, an interactive-independent acknowledgement flag, and parameterized checked-in SQL that begins a transaction, rechecks the expected row state/expired lease, changes only Telegram-owned retry/lease fields, and asserts one affected row.

The runbooks invoke the script rather than duplicating fragile SQL. Platform sessions are inspected/revoked only through Platform's authorized operator surface; the script never connects to Platform tables. Webhook registration and token revocation are separately labelled external writes and stop at a printed authorization boundary during dry-run. Candidate secrets are passed by file, validated without echoing content, installed atomically only in execute mode, and rolled back if role readiness fails.

Free-form SQL snippets and environment-secret arguments were rejected because they invite target mistakes and leak through shell history/process lists. Automatic recovery without prior state predicates was rejected because uncertain Bot API outcomes can otherwise duplicate delivery.

### 10. Validation is structural, executable, and evidence-bounded

Rust tests cover preference constraints/evaluation, binding authorization, duplicate/concurrent admission, rendering, queue priority/aging, NATS readiness/ack behavior, and content-free telemetry. Shell tests run every `telegram-ops` dry-run/read-only path against synthetic files, fake `systemctl`/`curl`/database adapters, validate unit directives/logrotate syntax, and extract/check runbook commands. The workspace profile supplies the cross-repository runtime proof.

Repository evidence claims only local tests, deploy structure, and dry runs. It cannot claim installed units, registered webhook, rotated real credentials, real-chat delivery, hosted CI, or live single-host operation without separate observed actions.

## Risks / Trade-offs

- **[Concurrent `/settings` and notification admission use different policy versions]** → Lock the policy row during admission and use optimistic version checks for commands; each decision stores the effective result.
- **[NATS disconnect occurs after database commit but before acknowledgement]** → Redelivery hits inbox/notification uniqueness, creates no second job, and can be acknowledged safely.
- **[Bot API send succeeds but the process dies before settlement]** → Existing uncertain-send retry semantics remain; notification identity prevents another queued job but cannot make Bot API itself idempotent, so the runbook surfaces uncertain attempts rather than declaring exact-once provider delivery.
- **[Priority aging changes historical strict FIFO]** → Preserve FIFO within each delivery class and one in-flight job per chat; cover interaction-first and bounded-aging cases explicitly.
- **[Root-owned rotation replaces a valid secret with a syntactically valid wrong one]** → Stage the old file, restart only affected roles, require readiness/provider verification, and provide atomic rollback before any provider revocation.
- **[The dispatcher depends on webhook for first schema application]** → Encode systemd ordering/retry behavior and make dispatcher schema check fail safely; development/CI databases are always created directly from current `schema.sql`.

## Migration Plan

1. Land and deploy the compatible Platform consumer topology/NATS identity first; do not start Telegram until the durable matches.
2. Build Telegram from an empty database in development/CI, add the NATS seed file and other root-owned credential files, and validate both units/configurations structurally.
3. Start webhook so it applies the current schema and becomes ready; then start dispatcher, which verifies the schema and durable before readiness.
4. Run the workspace TG-010 composed profile with synthetic credentials, article progress facts, notifications, and Bot API responses. No live host action occurs while the target is frozen.
5. On a future authorized live rollout, stage webhook secret registration, start both roles, verify private health/metrics, then enable producer traffic. Rotate/revoke old credentials only after the new path is observed.
6. To roll back, stop dispatcher notification consumption first, preserve its durable cursor and Telegram preference/decision rows, restore prior binaries/configuration, and revoke no credential until the restored roles are ready. During development, a fresh database is recreated from the prior `schema.sql`; no migration or compatibility layer is introduced.
