## Why

The operation follower currently marks a binding finished whenever one follow attempt returns, including transient owner lookup, session exchange, or event-stream failures. It also reuses one session across reconnects, so a live operation can stop projecting until the dispatcher process restarts.

## What Changes

- Distinguish terminal operation completion from retryable follow interruption and keep nonterminal bindings eligible for bounded resumption.
- Refresh the Platform session for each stream open or reconnect so expiry and credential rotation do not strand a live follower.
- Apply bounded backoff without permanently suppressing a live binding or running duplicate followers concurrently.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `article-capture`: live operations remain followed across transient authorization and stream failures without requiring process restart.

## Impact

- `services/dispatcher/src/follow.rs`, follower coordination state, session acquisition, and local Platform stream tests.
- No schema, public API, or dependency changes are expected.
