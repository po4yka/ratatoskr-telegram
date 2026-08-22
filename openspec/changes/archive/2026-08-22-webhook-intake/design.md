# Design: secure webhook intake

## Context

The scaffold runs two binaries with an operator plane and an empty `telegram` schema. `teloxide`
is pinned but unused; `DEVELOPMENT.md` records that the item taking it up owns moving it onto the
`cargo deny` surface. The webhook role's database is optional at this milestone, with a documented
promise: "when the first feature writes through the pool, this becomes a refusal to start". Item 2
is that first feature.

The legacy Telethon long-polling client is replaced by Bot API webhook mode. Telegram delivers
updates by POSTing to our endpoint; admission control — secret, limits, schema, dedupe — happens
before anything downstream sees the update.

## Goals / Non-Goals

Goals:

- No forged, oversized, malformed or redelivered update produces a side effect.
- Telegram receives its acknowledgment before any domain work starts, measurably.
- Every rejection class is typed, logged by safe class, and never echoes request content or secrets.
- Tests exercise recorded fixtures against a local harness server and the real PostgreSQL test
  database; no test contacts api.telegram.org.

Non-Goals:

- Command parsing, identity binding, access policy (item 3+); the worker classifies and settles
  state only.
- Durable cross-restart inbound queue and dead-letter machinery (the commands/events section of
  AGENTS.md places transactional outbox/inbox with item 4's dispatcher work). The crash window
  between persisting `accepted` and processing is recorded under Risks.
- Webhook registration automation (`set_webhook` exists as a client method; registration remains an
  explicit operational write done outside this process).
- An error envelope toward Telegram: it ignores response bodies; status codes are the contract.

## Decisions

### The client wraps teloxide rather than replacing it

`teloxide-core` 0.13 provides typed payloads, the `Update` taxonomy with battle-tested
deserialization, and error classes (`Api`, `RetryAfter`, `Network`). Re-deriving those would be
hundreds of lines duplicating a maintained crate. `ratatoskr-telegram-bot-api` exposes a narrow
facade — six methods now, `#[non_exhaustive]` errors — so callers never see teloxide types in
signatures except the re-exported update/message value types that ARE the wire vocabulary. The bot
token travels in the URL path (`/bot<token>/<method>`); the crate therefore never logs URLs.

`UpdateKind`'s custom deserializer maps unknown or kind-level-invalid payloads to
`UpdateKind::Error(Value)` instead of failing: exactly the split AGENTS.md requires between
"unsupported update type" (well-formed envelope, unknown kind — acked and recorded) and "malformed"
(envelope itself unparseable — acked and logged, nothing to record).

### Admission order is fixed and observable

One POST route. In order: secret header (constant-time via `subtle`, before reading the body),
method (405), content type (415), declared size from `Content-Length` (413), streamed read capped
at `max_body_bytes + 1` so chunked bodies cannot lie (413), JSON parse, dedupe insert, enqueue,
200. A body that parses but carries an unsupported kind is still accepted and recorded — it is
valid input this build does not act on.

Response codes are the whole interface: 200 accepted / deduplicated / malformed-acked; 401
unauthorized; 405; 415; 413 with a short plain-text limit body; 503 when storage fails or the queue
is saturated (Telegram retries both; nothing was committed).

### Deduplication is exact-match persistence, not a high-water mark

Telegram can redeliver old ids after reconnects. A `max(update_id)` watermark would reprocess them;
`(bot_id, update_id)` insert-or-ignore does not. Genuinely unseen ids below the high-water mark
(getUpdates gaps) are inserted and processed like any other. The row is written in the same step
that decides acceptance; the queue handoff reserves channel capacity BEFORE the insert
(`try_reserve`) so saturation 503s never leave an accepted-but-never-processed row behind.

Bot identity comes from `get_me` at webhook startup — required, exit 1 on failure. A webhook that
cannot name its bot cannot key its dedupe table and cannot serve; refusing is honest. The id is a
number, safe for logs and metrics.

### The queue is bounded and in-process

An `mpsc` channel (capacity 1024) into one worker task spawned at startup. The worker marks the row
`processing`, classifies the kind, settles it `processed` | `unsupported`, and logs failures as
`failed`. Items 3+ replace the worker body, not the intake contract. The task is detached: after
the grace window closes, queued items remain as `accepted` rows — visible evidence, not silent loss
(see Risks).

### Configuration grows two tables and role requirements

`bot_api`: `base_url` (default `https://api.telegram.org`; https enforced unless loopback, same
rule shape as OTLP V4), `timeout_seconds` 1..=60 default 10, `token` (`SecretString`, empty
default). `webhook`: `bind` (default loopback `127.0.0.1:9469`, continuing the allocation block),
`secret_token` (`SecretString`), `max_body_bytes` 1024..=1_048_576 default 262_144. New rules
append as V9–V13: V9 base-url scheme, V10 timeout bounds, V11 secret length 16..=256 over
`[A-Za-z0-9_-]` (Telegram's charset; the floor forces entropy), V12 body-cap bounds, V13 role
requirements — the webhook role requires `bot_api.token`, `webhook.secret_token` and a configured
database, and refuses equal admin/public binds. Reports stay value-free; both secrets render
`[REDACTED]`.

### One lifecycle, optionally two listeners

`telegram_http::run(role, public)` takes a `PublicRoutes` value: `none()` for the dispatcher, or a
factory receiving `{config, database}` and returning a `Router` — or a `TelegramError`, which
becomes the existing startup-failure path (log once, close what opened, exit 1). Both listeners go
through the same drain-then-close sequence. Unreachable-database refusal applies to the webhook
role only; the dispatcher keeps warn-and-degrade until its own item flips it.

### Metrics stay closed-vocabulary

`telegram_webhook_requests_total{outcome}` over the fixed outcome set above;
`telegram_updates_received_total{update_kind}` where kinds map to their wire keys and everything
unknown collapses to `other`; `telegram_webhook_duration_seconds` on the shared buckets. No
request-controlled string reaches a label.

## Risks / Trade-offs

- [In-process queue loses queued updates on crash between 200 and processing] → Rows persist as
  `accepted`, so the window is auditable; Telegram will not redeliver a 200. Accepted because the
  durable inbound path arrives with item 4's outbox/inbox work; shipping a second queueing system
  here first would be the stopgap.
- [teloxide pulls a large graph onto the audit surface] → That is the pinned intent of item 1's
  manifest comment; `cargo deny check` now covers it, which is strictly more audit than leaving it
  unused.
- [`UpdateId` is u32 while PostgreSQL gets bigint] → Lossless widening at the boundary; the column
  is bigint so a future Bot API widening is a type edit, not a migration.
- [Worker detached at shutdown] → Bounded by the queue capacity and grace window; documented
  behaviour, asserted nowhere as durability.

## Migration Plan

Not applicable — no deployed instance, no data to survive. Schema edited in place per development
status. Lands on a task branch, merges to `main` after the full gate is green.

## Open Questions

None.
