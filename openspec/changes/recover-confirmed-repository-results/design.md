## Context

Consuming a confirm token currently commits the dialogue to submitting before Platform HTTP. Platform may apply the action, after which dialogue completion and result enqueue are separate commits. A storage error becomes terminal update failure and removes the callback payload; the consumed token cannot authorize recovery.

## Goals / Non-Goals

**Goals:**

- Recover the exact confirmed action after restart without broadening callback replay authority.
- Atomically expose completed dialogue state and its result job.
- Preserve one Platform or provider mutation identity across uncertain responses.

**Non-Goals:**

- Reopening a consumed callback for a new Telegram update.
- Holding local transactions across Platform HTTP.
- Compensating or automatically reversing a successful provider mutation.

## Decisions

### D1: Bind submitting to the admitted update identity

The confirmation-consumption transaction records the bot and update identity that released the action. Reprocessing that exact retained update may resume while the dialogue is still submitting; another update presenting the same callback remains consumed and refused. A completed dialogue is observed as already settled and cannot submit again.

Reactivating the token was rejected because forwarded or replayed callbacks would regain authority. Creating a separate generic action table is unnecessary while the durable update already contains the bounded recovery input.

### D2: Completion and result enqueue share one transaction

A persistence operation validates submitting state and releasing update, updates the dialogue to completed, and inserts the structured result job atomically. Failure rolls back both. Unique dialogue and result identities make unknown commit convergence safe.

### D3: Post-confirmation errors are recoverable outcomes

Once confirmation is durably consumed, transient Platform uncertainty or local projection failure keeps the releasing update processable. Retry uses the existing dialogue idempotency key and confirmation evidence. Explicit provider refusal may complete with its truthful result.

## Risks / Trade-offs

- [A persistent database fault retains callback payload longer] -> Apply bounded retry and dead-letter policy and retain only the minimized fields already required to resume this service-owned workflow.
- [Platform returns different results for the same idempotency key] -> Treat that as a contract violation, record safe diagnostics, and never infer provider state.
- [Current schema changes] -> Schema fingerprinting makes old development databases fail closed; recreate rather than migrate.

## Migration Plan

Edit the current schema in place and recreate development databases. Deploy after the schema-fingerprint change so stale databases refuse startup. Rollback requires recreation from the rolled-back current schema and should occur only after webhook work drains.
