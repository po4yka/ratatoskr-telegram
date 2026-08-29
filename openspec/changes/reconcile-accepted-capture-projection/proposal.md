## Why

Platform can accept a capture before Telegram persists its operation binding, intent, and acknowledgement job. A later local storage failure currently terminally minimizes the source update, leaving an accepted Platform operation orphaned from Telegram with no restart recovery path.

## What Changes

- Persist the local projection of an accepted capture atomically: operation binding, opaque intent, and acknowledgement job either all commit or none do.
- Keep an accepted-but-not-yet-projected update processable instead of settling it terminally, so restart recovery reuses the same Platform idempotency key and converges.
- Preserve the fast terminal path for explicit Platform refusals and bounded failure before acceptance.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `article-capture`: an accepted capture must durably converge to exactly one bound acknowledgement after local faults and restarts.
- `webhook-update-recovery`: a claimed update must remain recoverable when an external acceptance is known but its local projection is incomplete.

## Impact

- `services/webhook/src/intake/capture.rs`, worker settlement outcomes, persistence transaction helpers, and PostgreSQL-backed capture tests.
- Platform submission remains outside database transactions and continues to use the existing stable idempotency key.
- No cross-repository contract or external dependency changes.
