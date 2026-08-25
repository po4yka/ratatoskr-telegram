# Design — dispatcher, message bindings, ordered/rate-limited delivery, operation projection

## Context

Items 1-3 left the service with an inbound half only: the webhook worker settles every admitted update terminally, and `services/dispatcher` is a stub that starts without a database. `crates/http/src/lib.rs` reserves the flip ("the dispatcher joins `role_requires_database` when its first write lands (plan item 4)"). The Bot API client already exposes `send_message`/`edit_message_text` over teloxide with a closed error taxonomy (`RateLimited { retry_after }`, `Api { description }`, `Network`, ...), and no test contacts Telegram — a local axum harness pattern exists in `crates/bot-api/tests/client.rs`. There is no NATS dependency; events arrive per the contracts store's envelope (`event_id`, `occurred_at`, correlation/causation ids) wrapping `platform.operation.progressed.v1` snapshots whose status enum is closed and whose stage is display-only text of producer origin.

Constraints that bind this design: development status (schema edited in place, one version); clippy.toml size limits (fn ≤ 100 lines, args ≤ 7, file ≤ 850); tests-first task pairs; hand-written fakes only; secrets and message content never logged.

## Goals / Non-Goals

**Goals:**

- One durable queue both runtime halves can trust: enqueue-then-deliver with explicit lifecycle, restart-safe.
- Deterministic timing behavior under test: injectable clock everywhere time matters.
- Failure handling that is honest per outcome class: nothing silently dropped, nothing uselessly retried.
- A consumer seam that later accepts NATS without changing its internal logic.

**Non-Goals:**

- NATS/JetStream client wiring (workspace integration item); priority scheduling between job classes (no producer until item 5 — noted as extension point); method-specific rate-limit classes beyond the single default (seam documented); callback/dialogue/commands/files; notification preferences; retention pruning loop implementation — the duty is stated and carried as `TODO(author)` with a named config key, since no data volume exists yet in development.

## Decisions

### D1 — Durable outbox table, not in-memory queue

`telegram.outbound_jobs` is the authority for outbound work, mirroring how `telegram.updates` already made PostgreSQL the intake authority. In-process queues lose work at restart and cannot enforce cross-restart ordering. State machine uses ARCHITECTURE.md §18.1's exact tokens: `planned → ready → sending → sent`, plus `retry_wait`, `superseded`, `failed_permanent`, `cancelled`.

*Uncertain-outcome semantics*: after a network timeout the send may or may not have arrived. The job is retried (at-least-once) with a bounded duplicate window — losing messages on routine blips is worse than a rare duplicate; edits self-heal because re-applying the same revision hits "message is not modified" = success. Stated in the spec, not improvised per call site.

### D2 — Per-chat-head claiming gives ordering for free

Claim query: newest-ready-per-chat via `SELECT DISTINCT ON (chat_id)` over eligible jobs ordered `(chat_id, id)`, `FOR UPDATE SKIP LOCKED`, one row returned per call. One in-flight job per chat means conflicting edits are impossible by construction, and N dispatcher replicas stay safe without redesign. Cross-chat order is explicitly unspecified; starvation is prevented because every chat's head is independently claimable.

### D3 — Guard precedence lives in one transactional accept step

Event acceptance order (spec: guard precedence): inbox insert-or-ignore on `event_id` → terminal flag check-and-set → `occurred_at` staleness → revision assignment. Revision is a per-binding persisted counter assigned at accept time — the Platform snapshot deliberately carries none, and inventing contract fields is refused. Enqueue-time supersede of older ready jobs is an optimization; the sender re-checks `binding.last_rendered_revision >= job.revision` at delivery, which closes the claimed-while-superseded race.

### D4 — Throttling is reschedule arithmetic, never timers

The consumer writes each accepted render as a job with `next_attempt_at = max(now, binding.last_rendered_at + min_interval)`. The sender enforces eligibility by timestamp comparison against the injected clock. Terminals skip only the interval delay, not chat serialization — worst-case latency is one in-flight API call, and queue-jumping stays impossible.

### D5 — Failure classification table maps the taxonomy once

| Outcome | Class | Action |
|---|---|---|
| success | success | settle `sent`; advance binding revision/message id |
| `message is not modified` | success-no-op | settle `sent`, advance revision |
| 429 + retry_after | rate-limited | reschedule now+retry_after+jitter; chat cooldown |
| network / timeout / 5xx-ish API | transient | backoff = base·2^attempt + jitter, cap; dead-letter after bound |
| bot blocked / chat not found / membership lost / message cannot be edited / edit target deleted / invalid markup / migrated-to-supergroup | permanent | settle `failed_permanent` immediately |
| permanent failure on an *edit* | unbind-and-resend | clear binding's message id; next revision sends fresh message and rebinds (§19 fallback) |

Classification is a pure function from `BotApiError` (+ description matching) so the table is unit-testable without I/O.

### D6 — Transport seam for events

The consumer consumes `OperationProgressed` values (typed struct mirroring the published contract's required fields) through a channel-backed seam whose documented contract is at-least-once/no-ordering. Subject constants cite the contracts store's `evt.*` grammar in docs; the JetStream adapter lands with workspace integration and must not change consumer internals. Duplicate delivery is proven through the seam itself.

### D7 — Send path is generic; auto-create stays out

A send job may carry an operation reference; the sender establishes the binding after Bot API ack ("store provider message IDs only after successful response"). The projection consumer is edit-only into existing bindings — otherwise every Platform operation would message the chat unsolicited. Commands (item 5) create the initial ack send.

### D8 — Code layout follows the webhook precedent

- `services/dispatcher/src/lib.rs` + modules: `outbound/` (limiter, classify, sender), `projection/` (render, consumer), `build.rs` startup factory spawning workers after DB connect.
- `crates/persistence`: new `message_bindings.rs`, `outbound_jobs.rs`, `inbox.rs` repositories; schema tables appended to `schema.sql`.
- `crates/core`: `DispatcherConfig` (global msg/s, per-chat interval, render interval, attempt bound, lease TTL, backoff base/cap, jitter fraction) with validation rules continuing the V-numbering; unknown keys refused as everywhere else.
- `crates/http`: `role_requires_database` gains `Dispatcher`; boot tests updated to: absent DB config → refuse to start; configured-but-unreachable → refuse; DB dies later → readiness fails, process stays up (existing prober semantics).
- `crates/telemetry`: `telegram_delivery_duration_seconds`, `telegram_delivery_retries_total{class}`, `telegram_rate_limit_waits_total`, `telegram_projection_lag_seconds`, `telegram_outbound_queue_depth{state}`, `telegram_delivery_failures_total{class}`, `telegram_projection_events_total{outcome}` — README-published names reused verbatim where they exist; labels are closed safe classes only.
- Clock injection: a thin `Clock` trait (now/instant) threaded into limiter/sender/consumer; production uses the tokio clock, tests control it — paused-time assertions without sleeps.

### D9 — Schema shapes follow house conventions

App-minted UUIDv7 ids with no DEFAULT; closed vocabularies as `text`+CHECK (`kind`: `send_message`/`edit_message_text`; job state machine tokens; `last_delivery_status` mirrors outcomes); all timestamps `timestamptz`; `bot_id` carried on jobs and bindings for multi-bot correctness; uniqueness: one live binding per `(operation_id, chat_id)`, inbox PK `event_id`; FKs never cross schemas. Doc drift noted: AGENTS.md says `outbound_jobs`, ARCHITECTURE.md §5 says `outbound_messages` — AGENTS.md's name wins here; ARCHITECTURE.md §5 is a target sketch, not a contract.

## Risks / Trade-offs

- [Duplicate user-visible message after crash-between-send-and-ack] → bounded by lease TTL and documented at-least-once semantics; edits self-heal; window is seconds, not minutes.
- [`DISTINCT ON` claim under contention] → single dispatcher deployment initially; SKIP LOCKED keeps replicas correct if scaled; queue-depth metric exposes starvation.
- [Stage/error text from producers could carry markup or length bombs] → HTML escape + Telegram 4096-char truncation in the renderer; injection tests pin it.
- [Revision counter concentrates correctness in one column] → guarded transactionally at accept and claim; staleness check adds a second, independent line of defense.
- [Boot-behavior change surprises operators running dispatcher without DB] → DEVELOPMENT.md and README updated in the same change; validation error names the missing section like every other required key.

## Migration Plan

No database holds data that must survive a schema change: fresh databases get the three tables from the edited `schema.sql`; nothing migrates. Rollback is reverting the branch; the only external effect is dispatcher processes refusing to start without configuration, which is the documented trajectory. Deployment note for the runbook (docs update in this change): dispatcher requires `RATATOSKR__DATABASE__URL` from this item onward.

## Open Questions

None blocking. Retention policy numbers and NATS consumer options belong to their own items.
