# Dispatcher, message bindings, ordered/rate-limited delivery, and operation projection

## Why

Plan items 1-3 admit, authorize, and settle updates, but nothing ever sends a Telegram message back: the service has no outbound half. Plan item 4 of `docs/IMPLEMENTATION_PLAN.md` builds it - a dispatcher that delivers projections durably and truthfully - so later items (commands, files, GitHub flows) only have to produce intents and events, never Bot API plumbing.

Scope interpretation, stated so plan text cannot be misread: "dispatcher consuming authorized updates into typed intents" is satisfied by the typed internal representations this change introduces (`OutboundJob`, projection revisions) plus the outbound machinery that consumes them. Update-to-interaction-intent conversion stays with the webhook worker until item 5 gives commands something real to convert into; this change moves no update-settlement behavior.

## What Changes

- `schema.sql` gains three tables, edited in place per development status: `telegram.message_bindings` (operation -> chat/message binding with monotonic render state), `telegram.outbound_jobs` (the durable send/edit queue with its state machine), `telegram.inbox` (event-id deduplication for at-least-once event consumption).
- The dispatcher becomes a real runtime: a library beside its binary (mirroring the webhook), a startup factory that connects PostgreSQL (now required at startup, matching the reserved hook in `crates/http/src/lib.rs`), an outbound sender worker, and an operation-event consumer.
- Outbound delivery is a durable queue: jobs carry bot/chat/kind/payload/revision/correlation ids; claiming is strict FIFO per chat with one job in flight per chat and no cross-chat ordering promise; sends and edits both pass a global token bucket and a per-chat minimum interval; `Retry-After` reschedules authoritatively; transient failures retry with capped jittered backoff to a bounded attempt count; permanent failures dead-letter as `failed_permanent`.
- Edits are idempotent: a job whose revision is not newer than the binding's last rendered revision is superseded without an API call; a rendered-content hash turns identical re-renders into success no-ops; the Bot API's `message is not modified` answer is treated as success.
- An operation-projection consumer accepts Platform `platform.operation.progressed.v1` snapshots over a transport seam (NATS wiring arrives with workspace integration; subjects follow the contracts store's `evt.*` grammar). It deduplicates by envelope `event_id`, guards precedence inbox-dedup -> terminal flag -> staleness -> revision, assigns per-binding revisions, renders status/stage into HTML-escaped progress text, and enforces the per-binding minimum edit interval by durable reschedule arithmetic - terminals flush immediately but never jump the chat queue.
- Terminal states render exactly once: the terminal flag is set transactionally when the terminal job is inserted; second terminals and post-terminal events are dropped with class-only metrics.
- A permanent edit failure (message deleted, cannot be edited) unbinds; the next revision sends a fresh message and rebinds instead of killing all rendering for the operation.
- Configuration gains the `RATATOSKR__DISPATCHER__*` section with validated limits; telemetry gains delivery/retry/rate-limit/projection metrics under the names README already publishes; boot tests now expect the dispatcher to require database configuration like the webhook does.

Out of scope, unchanged from the user's direction: command parsing and URL/article flow (item 5), file handling (item 6), callback/dialogue machinery (item 8), NATS transport itself (workspace integration). Notification preferences are untouched.

## Capabilities

### New Capabilities

- `outbound-delivery`: Durable ordered/rate-limited Bot API delivery - job lifecycle, per-chat FIFO ordering, global/per-chat rate limits, retry and failure classification, idempotent edits, restart recovery.
- `operation-projection`: Rendering Platform operation snapshots into bound Telegram messages - event deduplication, guard precedence, revision assignment, throttled edits, terminal-once semantics, HTML escaping.

### Modified Capabilities

(none - `openspec/specs/` is empty; the cross-repository producer side of `platform.operation.progressed.v1` stays governed by the ratatoskr-workspace store's `operation-progress` spec, which this change cites and does not restate.)

## Impact

- `schema.sql` - three new tables following its conventions (app-minted UUIDv7 ids, `text`+CHECK vocabularies, `timestamptz`).
- `crates/persistence` - new repository modules for bindings, outbound jobs, and the inbox.
- `services/dispatcher` - from stub binary to library + workers; requires database at startup.
- `crates/core` config model/validation - new `DISPATCHER` section and rules.
- `crates/http` - `role_requires_database` includes `Dispatcher`; boot expectations change accordingly.
- `crates/bot-api` - unchanged surface; consumed by the sender through its existing error taxonomy.
- `crates/telemetry` - new metric constants; no identifiers in labels.
- Docs: README status, DEVELOPMENT.md stage/commands, boot tests in `services/webhook/tests/boot.rs` that pin dispatcher startup behavior.
