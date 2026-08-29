## Why

A confirmed GitHub action can succeed in Platform after its callback is consumed, then lose its Telegram result when dialogue completion or outbound enqueue fails locally. The callback payload is currently minimized as failed, so the durable confirmation cannot resume and the dialogue can remain stuck or silently complete without a user-visible result.

## What Changes

- Bind the submitting dialogue to the durable Telegram update that consumed its confirmation and allow only that same update to resume recovery.
- Commit dialogue completion and the outbound result job in one local transaction.
- Keep post-confirmation local failures processable so recovery reuses the same action idempotency key without authorizing a foreign or replayed callback.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `github-repository-flow`: a confirmed action must recover its result projection after local faults without a second provider mutation identity.
- `dialogue-state`: the submitting state records which durable update owns recovery, while other callback replays remain rejected.
- `webhook-update-recovery`: post-confirmation storage failures retain the update payload until completion and result enqueue converge.
- `persistence-schema`: current-schema dialogue authority stores the durable update identity used for recovery.

## Impact

- Current `schema.sql`, repository-dialogue persistence, GitHub callback handling, worker settlement outcomes, and PostgreSQL-backed tests.
- No database migration is added; development databases are recreated after the in-place schema edit.
- Platform action submission keeps its existing idempotency contract and remains outside database transactions.
