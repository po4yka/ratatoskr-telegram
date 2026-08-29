## Context

Capture processing calls Platform first, then separately finds or creates a binding, issues an intent, and enqueues an acknowledgement. A failure after Platform acceptance is mapped to terminal failure, and terminal settlement removes the only processable update payload. The submission already has a deterministic idempotency key, so the durable update can be the recovery authority without a second command framework.

## Goals / Non-Goals

**Goals:**

- Make the local accepted-capture handoff atomic and restart-recoverable.
- Reuse the same external command identity after every uncertain local boundary.
- Preserve exactly one acknowledgement binding per operation and chat.

**Non-Goals:**

- Holding a PostgreSQL transaction across Platform HTTP.
- Claiming atomicity across Platform and PostgreSQL.
- Generalizing every webhook command into a new workflow engine.

## Decisions

### D1: One transaction records the accepted projection

A persistence operation receives the accepted operation identity, chat and owner scope, intent fields, and acknowledgement payload. It inserts or resolves the binding, lets only the winning unacknowledged binding create the intent and job, and commits all effects together. Existing transaction-aware insert helpers are reused.

Separate idempotent calls were rejected because an error between them still exposes partial state and makes the caller reconstruct transaction ownership.

### D2: Processing distinguishes retry from terminal settlement

The capture handler returns an explicit recoverable outcome after external acceptance when local projection is incomplete. The worker releases or lets the lease expire back to processable state without terminal minimization. Explicit Platform refusal and bounded failure known to precede acceptance retain terminal behavior.

Mapping every error to failed was rejected because it destroys the durable replay input exactly when it is needed.

### D3: Re-submit with the original idempotency key

Recovery recomputes the same key from the retained update and repeats Platform submission outside the database transaction. Platform returns the same operation for an already accepted command, after which the local transaction converges. Unknown local commit is handled by the same unique constraints and winner logic.

## Risks / Trade-offs

- [Platform violates its idempotency contract] -> Keep the existing stable key and test every observed retry uses it; do not compensate in Telegram.
- [A poison local error retries forever] -> Preserve bounded attempts, backoff, and dead-letter inspection, but do not minimize the payload as ordinary failure while accepted projection can still recover.
- [Concurrent claims see one accepted operation] -> Existing update leasing plus transaction uniqueness ensures one projection winner.

## Migration Plan

No schema migration or cross-repository rollout is required. Deploy the webhook code and transaction helper together. Rollback restores the prior behavior but can again orphan newly accepted captures, so rollback is only safe after draining webhook work.
