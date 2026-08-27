## Why

Item 7 introduced a GitHub-specific callback authority while capture deep links use a separate, read-only intent record and no durable dialogue state exists. Item 8 replaces those ad-hoc seams with one reusable, replay-safe interaction authority before more multi-step bot and Mini App flows depend on them.

## What Changes

- Add a generalized registry that mints high-entropy opaque callback and deep-link tokens, keeps action/payload/scope server-side, enforces expiry and one-time consumption transactionally, and returns a stable expired-state result for replay without re-executing the action.
- Reconcile the GitHub repository selection/confirmation flow onto the generalized registry while preserving its owner/bot/chat/message/version checks and stable action idempotency identity.
- Add durable versioned dialogue state for awaiting-input flows, including expected-step transitions, cancellation/completion, timeout-to-expired behavior, and restart-safe reads.
- Parse Telegram `/start <opaque-token>` payloads only as deep-link intent tokens; resolve them under the presenting bot/user/chat scope and never interpret the payload as business data.
- Extend interaction intents with explicit one-time consumption/replay evidence and typed server-side action payloads.
- Add a bounded cleanup operation/job that expires live stale dialogues and removes aged consumed/expired token records without deleting domain operations or other service-owned state.
- Edit the first-version schema in place and remove the item-7-only callback storage paths after all callers use the generalized authority; no migration or compatibility path is added.

## Capabilities

### New Capabilities

- `interaction-token-registry`: Shared opaque token issuance, scoped resolution, transactional single-use consumption, replay refusal, and stale-token cleanup for callbacks and deep links.
- `dialogue-state`: Durable awaiting-input state machines with optimistic transitions, cancellation/completion, expiry, and stale-state cleanup.

### Modified Capabilities

- `github-repository-flow`: Route existing repository selection, confirmation, cancellation, and replay behavior through the generalized token/dialogue authority.
- `article-capture`: Resolve `/start` deep-link payloads as scoped opaque operation intents with one-time replay behavior.
- `persistence-schema`: Replace item-specific callback storage and read-only intent semantics with the generalized token registry and durable dialogue state in the current schema.

## Impact

- Affected surfaces: webhook callback/start handling, GitHub repository confirmation, dialogue transitions, deep-link rendering/resolution, dispatcher intent lookup, persistence cleanup, telemetry, current schema, and synthetic Bot API/database tests.
- No cross-repository contract, provider API, Mini App authentication, new command family, production credential, or new database version is introduced.
- Existing item-7 callback tables and persistence APIs are deliberately removed once their callers move to the generalized first-version model.
