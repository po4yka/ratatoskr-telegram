## Context

The follower keeps process-local in-flight and finished sets. Its wrapper adds an operation to finished whenever one follow attempt returns, even when return followed transient owner, session, or stream failure. The scanner then suppresses that still-live binding forever. Session acquisition happens once before the reconnect loop, so reconnect can reuse expired credentials.

## Goals / Non-Goals

**Goals:**

- Treat durable terminal binding state as the only completion authority.
- Resume every retryable failure without dispatcher restart and without concurrent duplicate followers.
- Refresh session credentials at every stream-open boundary.

**Non-Goals:**

- Persisting a second follower state machine outside message bindings.
- Removing event replay or inbox deduplication.
- Retrying terminal authorization or refusal classes indefinitely.

## Decisions

### D1: Remove process-lifetime finished state

The in-flight set remains the concurrency guard while a task owns an operation. On task exit it is always cleared. Later scans consult the binding's durable terminal flag; any nonterminal binding may be scheduled again after backoff.

Returning richer terminal or retry enums while retaining finished was rejected because two authorities can diverge again.

### D2: Bound attempts per task and resume via later scans

A follow task uses bounded reconnects and backoff to avoid monopolizing a worker. Exhaustion returns the operation to eligibility rather than marking it complete. Last-Event-ID plus inbox deduplication makes frame replay safe.

### D3: Acquire a valid session for each open

Owner resolution and session acquisition occur before every initial open and reconnect. Authentication rejection invalidates only the exact cached credential that was rejected, avoiding deletion of a newer concurrently refreshed session. The next attempt exchanges again.

## Risks / Trade-offs

- [A permanently broken live binding retries indefinitely] -> Bound each attempt, use the fixed scan backoff and safe-class telemetry, and retain operator inspection.
- [Repeated scans race] -> Insert into in-flight before spawn and remove it exactly once on exit.
- [Reconnect duplicates frames] -> Preserve last event id and existing transactional inbox deduplication.

## Migration Plan

No schema or cross-repository rollout is required. Implement under the owned dispatcher runtime so shutdown cancellation removes in-flight state without claiming terminal completion. Rollback restores restart-dependent recovery.
