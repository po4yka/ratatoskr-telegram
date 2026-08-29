## Context

The Bot API acknowledges a send before the dispatcher persists message identity, binding effects, and job settlement. Those writes are separate today. Any failure leaves a sending lease that stale recovery reclaims and sends again. Telegram exposes no idempotency or lookup key for reconciling sendMessage, so a crash after the provider applied it cannot be proven either delivered or not delivered.

## Goals / Non-Goals

**Goals:**

- Never automatically duplicate a non-idempotent send after an ambiguous outcome.
- Commit all known-acknowledgement local effects atomically.
- Preserve bounded retries where provider evidence says the request was not applied.

**Non-Goals:**

- Claiming exactly-once delivery across Telegram and PostgreSQL.
- Automatically choosing between possible message loss and duplication after process loss.
- Changing safe edit idempotency semantics.

## Decisions

### D1: One acknowledgement transaction

A payload-specific persistence operation records the returned Telegram message id, callback stamp, binding creation or update, render revision, notification outcome, and job sent state in one transaction. After the wire call returns success, the worker retains the response and retries only this idempotent transaction through its bounded grace policy.

Independent writes were rejected because they can expose a binding without settlement or settlement without required projection effects.

### D2: Stale sends become outcome unknown

Stale recovery differentiates operations. An expired non-idempotent send with no durable acknowledgement transitions to a terminal quarantine state and is not generally claimable. Expired edits may be reclaimed because applying the same edit again does not create a second message.

Automatic resend was rejected because Telegram cannot reconcile it and duplicate user-visible messages are a real correctness failure. Treating the job as failed was also rejected because failure is not known.

### D3: Retry only definite not-applied outcomes

Telegram API responses that explicitly refuse without application, including eligible rate-limit responses, follow current bounded retry policy. Transport timeout, connection loss after write, process death, or malformed success handling is ambiguous and quarantined.

### D4: Recovery is explicit and warning-bearing

Operator inspection shows safe identifiers and the unknown class, not message content. A future or manual resend requires an execute flag, transactional state recheck, and an explicit duplicate-risk warning; it creates a new deliberate attempt rather than pretending to resume exactly-once delivery.

## Risks / Trade-offs

- [An ambiguous request was not delivered] -> Prefer honest quarantine over silent duplicate risk and expose explicit recovery.
- [Process dies before the known acknowledgement transaction] -> This remains fundamentally unknowable and is classified unknown on lease expiry.
- [Schema state confuses older binaries] -> Development uses one definition and database recreation; no mixed-version compatibility is promised.

## Migration Plan

Edit the current schema and recreate development databases. Deploy after owned dispatcher shutdown so known acknowledgements get their drain opportunity. Rollback requires schema recreation and reintroduces automatic replay risk, so inspect all quarantined jobs first.
