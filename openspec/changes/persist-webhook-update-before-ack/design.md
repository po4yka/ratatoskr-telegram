## Context

See `proposal.md` for motivation. The webhook currently inserts a deduplication row and places the only parsed `Update` in a bounded `mpsc` channel. The channel is useful for wake-up and backpressure but cannot be durable authority.

## Goals / Non-Goals

**Goals:**

- Recover every authenticated accepted update after process restart.
- Keep deduplication and terminal settlement exactly-once at the database boundary.
- Remove private Telegram payload data as soon as processing reaches a terminal state.

**Non-Goals:**

- No command handlers, dispatcher, Bot API call, new dependency, migration, raw unauthenticated body retention, or deployment.

## Decisions

### Persist the parsed update as restricted JSONB until settlement

The parsed `teloxide::types::Update` is the existing worker input and already excludes malformed or unauthenticated bodies. Serializing it avoids a second shadow schema that would lose fields future handlers require. The payload column is nullable and cleared at terminal settlement.

Alternative: retain only `kind`. Rejected because it can reproduce today's classification but cannot recover the accepted interaction once command handling uses message or callback fields.

### PostgreSQL is the queue authority; `mpsc` is a wake-up hint

Admission inserts the payload before success. The worker repeatedly claims pending rows from PostgreSQL and can be nudged by the existing bounded channel. Startup and periodic drain discover rows whose notification was lost. A compare-and-set claim prevents duplicate processing; interrupted `processing` rows are eligible for recovery according to the existing single-process restart model.

Alternative: replay rows into the channel only at startup. Rejected because a crash can still occur after a live insert and before the send.

### Clear payload in the terminal state update

The same SQL statement that records `processed`, `unsupported`, or `failed` sets the payload to null. The identity, kind, state, and timestamps remain for deduplication and audit. No private message content is kept after settlement.

### Edit `schema.sql` in place

The repository is in development and explicitly forbids migrations. PostgreSQL 17 integration tests create the schema from scratch.

## Risks / Trade-offs

- [A pending row temporarily holds private message content] → Store only authenticated parsed data, never log it, restrict it to the private schema, and clear it atomically on settlement.
- [A process can stop while a row is processing] → Make processing claims recoverable after restart and keep handlers idempotent.
- [Polling can add load or latency] → Drain on notification and use one bounded fallback interval; one service process and indexed pending state keep the query small.

## Migration Plan

Run schema and crash-recovery integration tests against a fresh PostgreSQL 17 database, commit on `main`, and push after the repository gate. Rollback is a commit revert plus development database recreation. No provider or frozen-host state changes.
