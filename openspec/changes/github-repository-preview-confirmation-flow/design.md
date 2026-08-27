## Context

See `proposal.md` and the three delta specs. The webhook worker already receives durable callback-query updates through the access gate, the Bot API crate can answer callback queries, outbound payloads carry inline keyboards, and the Platform client/session source authenticates as the Telegram user. Platform's accepted gateway forwards `/v1/gh` to GitHub and GitHub supplies the shared contract. Generic article parsing currently claims every bare HTTP(S) URL first.

## Goals / Non-Goals

**Goals:**

- Route exact repository URLs before generic capture without changing explicit `/summarize` semantics.
- Persist the smallest safe selection/confirmation flow that item 8 can later generalize.
- Guarantee no action client call before one valid owner-bound confirm transition wins.
- Render only shared preview/result facts and safe failure classes.

**Non-Goals:**

- General dialogue state, Mini App forms, account/list selection, OAuth, unstar, or operation-event following for these synchronous control actions.
- Replacing the existing deep-link intent table or merging its distinct lifetime semantics into callback confirmation state.

## Decisions

### D1: Repository parsing precedes generic bare-URL capture

A small pure parser accepts exact canonical repository URLs; the worker tests it before the article parser for ordinary messages. `/summarize` remains explicit content capture and forwarded multi-link behavior remains unchanged. Treating all `github.com` paths as repositories was rejected because issue, pull, release, and file URLs are valid content inputs.

### D2: Extend `platform-api` with typed gateway calls

The client posts preview/action contract bodies to `/v1/gh/repositories/...` with the same cached Platform bearer session used for captures. A hand-written fake HTTP harness records calls and injects contract results. Direct GitHub service calls were rejected because they bypass Edge identity and route policy.

### D3: Use one flow row plus opaque transition-token rows

`callback_flows` stores owner/chat/bot, stable target/account, selected mode, stage/version, expiry, expected provider message, and the stable action idempotency key. `callback_tokens` stores app-minted random identifiers and closed transition actions. Consumption locks both token and flow, checks owner/chat/message/stage/version/expiry, marks one token consumed, and advances the flow; this gives confirm/cancel/replay one winner without building generic dialogues.

Preview buttons are `select_mode` tokens. A winning selection creates distinct `confirm_action` and `cancel_action` tokens. Callback data is only the token string.

### D4: Bind callback flows to the Bot API message after provider acknowledgment

Outbound jobs gain an optional callback-flow reference. When a preview/confirmation send succeeds, the existing sender stamps the returned Telegram message ID into that flow. Callback consumption requires the message binding. This avoids trusting a forwarded button and preserves the rule that provider IDs are stored only after provider acknowledgment.

### D5: Answer the callback before the domain call

The worker calls the existing `answer_callback_query` immediately after ownership/token validation and before preview/action network work, then enqueues the next/result message through the durable outbound queue. Failure to stop the spinner is telemetry/UX failure and does not turn a valid confirmed transition back into an unconfirmed one.

### D6: Confirmation consumption and action identity are durable before submission

The winning confirm transaction changes the flow to `submitting` and fixes its idempotency key before any HTTP request. Transient/uncertain responses retry only that request identity. The terminal shared result is persisted on the flow before its message is enqueued, so restart recovery cannot invent success or submit a new provider-write identity.

### D7: Compose component rows directly from the shared contract

Rendering has exhaustive matches for metadata, provider star, and desired backup outcomes. Optional preview fields are omitted. HTML escaping uses the existing controlled renderer. No local aggregate inference overrides GitHub's validated result.

## Risks / Trade-offs

- [Preview buttons remain visible after one is consumed] -> Every token is one-time and flow-version checked; stale presses get a minimal answer and no action. Later item 8 may add keyboard cleanup without changing authority.
- [Callback answer fails while action succeeds] -> Record the Bot API failure class but preserve the already-confirmed action/result truth.
- [Crash after confirm commit and before HTTP response] -> Recover `submitting` flows and retry the same idempotency key; never mint another write identity.
- [GitHub contract pin cannot be fetched] -> Do not use a temporary path dependency in commits; wait for the contracts commit to exist remotely, then pin its immutable SHA.

## Migration Plan

1. Rebase the task worktree on current Telegram `main` after the GitHub producer is merged and live-gate green.
2. Pin the merged contracts SHA.
3. Add one RED/GREEN pair at a time: URL routing, preview rendering, selection gate, confirm consumption, replay/ownership, and partial rendering.
4. Edit `schema.sql` in place and update schema tests; add no migration file.
5. Run targeted tests, then the full gate through `build-gate`.
6. Start the disposable GitHub fake-provider service and exercise the Telegram fake Platform/Bot API path before merge.

Rollback disables GitHub URL routing first; unconsumed callback rows expire. Confirmed provider results remain visible/auditable and are never automatically reversed.
