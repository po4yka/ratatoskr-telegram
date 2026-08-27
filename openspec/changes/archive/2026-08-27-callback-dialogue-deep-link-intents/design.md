## Context

See `proposal.md` for motivation. The current code has two persistence models: `callback_flows` plus `callback_tokens` implement only the GitHub repository confirmation sequence, while `interaction_intents` stores UUID deep-link references that expire but cannot be consumed. The webhook worker is the single owner of inbound interaction processing and already routes callbacks outside the fast acknowledgment path. PostgreSQL is authoritative; the queue is only a wake-up hint.

The first-version schema is recreated from `schema.sql`, so this change edits it in place and removes superseded tables and APIs. No data migration or compatibility layer is allowed. Network side effects remain outside database transactions.

## Goals / Non-Goals

**Goals:**

- Give callbacks and `start` deep links one issuance/validation/consumption vocabulary with typed refusals.
- Represent GitHub confirmation as one durable dialogue with optimistic version transitions.
- Make expiry and cleanup deterministic from caller-supplied timestamps and bounded database work.
- Preserve the existing GitHub target, confirmation, and idempotency guarantees while deleting its ad-hoc store.

**Non-Goals:**

- Mini App `initData` validation, Platform assertion exchange, or general Mini App APIs (item 9).
- New business commands or dialogue kinds beyond the existing GitHub flow and operation-status deep-link intent.
- Provider calls, domain-owned state, generic workflow scripting, migrations, or compatibility routing.

## Decisions

### 1. Replace the three item-specific tables with two generalized tables

`telegram.dialog_states` owns finite-state interaction context:

- app-minted UUIDv7 id;
- bot/user/chat scope and optional provider-acknowledged message id;
- closed `kind` (`github_repository`) and lifecycle (`active`, `completed`, `cancelled`, `expired`);
- closed current step for the implemented kind, monotonic version, expiry, stable action idempotency key, and bounded object payload;
- created/updated/terminal timestamps.

`telegram.interaction_tokens` owns client-presented authority:

- a URL-safe token primary key with no database default;
- closed surface (`callback`, `deep_link`) and typed action;
- bot/user/chat scope, optional expected message, optional dialogue id/version, and optional operation reference;
- bounded typed payload, expiry, and paired `consumed_at`/`consumed_by_user` evidence.

The implementation removes `callback_flows`, `callback_tokens`, and `interaction_intents`, then updates every caller in the same change. Rust enums with `deny_unknown_fields` decode each dialogue/token payload before it crosses the persistence boundary; SQL constrains the surface/action/kind/lifecycle vocabularies and JSON object shape. This keeps one authority model without making a free-form automation store.

Alternative considered: keep all three existing tables and add a repository facade. Rejected because scope/version/expiry behavior would remain duplicated and item 7's repository columns would still define the supposedly general dialogue model.

### 2. Mint exactly 64 URL-safe characters from existing UUID randomness

Enable UUID v4 on the existing pinned `uuid` dependency. Concatenate three independently generated UUIDv4 byte arrays and encode the 48 bytes with unpadded URL-safe Base64. The resulting token is exactly Telegram's 64-byte callback-data ceiling and contains roughly 366 random bits after UUID version/variant bits, exceeding the required 256-bit entropy without adding a production package.

The raw token is stored because the dispatcher must recover an unconsumed operation-status token by operation id when it renders a later terminal message. Tokens are capability-like but not credentials: complete server-side scope checks and short expiry remain mandatory.

Alternative considered: hash tokens at rest. Rejected for this slice because a hash-only record cannot reconstruct the delayed deep-link button; adding encrypted token recovery would introduce a new secret-management boundary unrelated to item 8.

### 3. Consume token and advance dialogue in one transaction

The repository selects the token and referenced dialogue `FOR UPDATE`, then checks in this order: presence/grammar, consumed state, expiry, complete bot/user/chat/message scope, dialogue lifecycle/step/version, and typed action/payload. A successful one-time consume records consumption and applies the compare-and-swap dialogue transition before commit. The returned action is executed only after commit.

All unusable recognized callback presentations map to one outward response, `This action has expired. Please start again.`, and always call `answerCallbackQuery`; internal typed refusals remain distinct for tests and bounded telemetry. Scope mismatch does not consume the token, preventing a foreign press from denying the owner's valid action.

Alternative considered: update the token first and the dialogue in a second transaction. Rejected because a crash or competing confirm/cancel could consume authority without advancing state, or advance the same state twice.

### 4. Model GitHub item 7 as the first dialogue kind

The existing preview payload becomes a closed `GitHubRepositoryDialogue` value stored behind `kind=github_repository`. Steps remain `preview -> confirming -> submitting -> completed`, with `cancelled` and `expired` terminal exits. Selection consumes a version-0 token, stores the selected mode, advances to version 1, and mints confirm/cancel tokens. Confirm/cancel consume version-1 authority; confirmation advances to submitting/version 2 and returns the stable request identity. Provider result completion advances to completed/version 3 outside the token transaction but only from submitting.

The webhook's outward cards and Platform calls stay unchanged except the shared expired-state response. This is an internal contract break only: no GitHub or Platform wire shape changes.

### 5. Treat `/start` payloads as token grammar before URL parsing

The message parser recognizes exactly `/start <64 URL-safe characters>`. It produces a `DeepLinkToken` value, not an operation id, URL, or action. The worker passes bot/user/chat plus the token to the registry, whose successful transaction returns the closed operation-status intent once. Malformed payloads never reach a business parser or database action.

Capture acceptance continues to mint the deep-link token with its operation reference and bounded source/blob/forward presentation payload. Dispatcher lookup by operation obtains the still-live raw token for link composition; the URL uses `https://t.me/<bot>?start=<token>`. Consumption changes only Telegram interaction authority and never the Platform operation.

Alternative considered: retain `startapp=<uuid>` until Mini App auth arrives. Rejected because this item explicitly owns `start` payload parsing and item 9 is the security boundary that will define Mini App launch/session consumption.

### 6. Run cleanup inside the existing webhook worker lifecycle

The persistence API exposes one bounded cleanup transaction taking `now`, batch size, and terminal-retention cutoff. It first marks eligible live dialogues expired with a version increment, then deletes eligible stale tokens, then deletes retention-expired terminal dialogues after their tokens are gone. Stable ordering and a fixed maximum keep locks and connection time bounded.

The webhook worker runs one pass before claiming updates and schedules later passes from a monotonic fixed interval while continuing to use wall-clock seconds only for database cutoffs. Keeping cleanup in the existing worker avoids another detached task and pool consumer. Tests drive the repository pass with explicit timestamps; they do not sleep.

Alternative considered: a separate cleanup binary or cron job. Rejected because no deployment scheduler exists and cleanup is small, Telegram-owned interaction work.

## Risks / Trade-offs

- [A 64-character callback token uses Telegram's full callback-data allowance] -> Keep the token as the entire payload and add boundary tests against exact serialized byte length.
- [Raw opaque tokens are recoverable from the database] -> Keep expiry short, require full actor/chat/message scope, never treat a token alone as authorization, and exclude token values from logs/metrics.
- [Generic JSON could become an unbounded business-state bag] -> Use one closed enum per kind/action, deny unknown fields, enforce object shape and repository size checks, and store references/projection facts only.
- [Cleanup racing with consumption can alter the refusal class] -> Both paths lock rows transactionally; outward behavior intentionally converges on the same expired-state message and no action.
- [The schema replacement breaks existing dev databases] -> Development status explicitly has no durable data; recreate the database from the one current `schema.sql` and do not add migration/compatibility code.

## Migration Plan

1. Change `schema.sql` in place and update current-schema tests; recreate disposable test databases.
2. Add generalized persistence repositories and tests, then move GitHub and operation-intent callers in one branch.
3. Remove the old persistence modules/tables only after repository-wide call-site search is empty.
4. Run targeted behavior/database tests, the repository's complete `DEVELOPMENT.md` gate, strict OpenSpec validation/archive, and verify the final diff.
5. Rollback, if needed before release, is a source revert plus recreation of the development database from the reverted current schema; no production data conversion is claimed.
