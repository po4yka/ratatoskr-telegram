# URL/article capture command and message flow

## Why

Plan items 1-4 admit, authorize, and project updates, but no user-visible product exists yet: nothing turns a message into domain work. Plan item 5 of `docs/IMPLEMENTATION_PLAN.md` builds the first product slice from `docs/REQUIREMENTS.md` - an authorized user sends an article URL and receives one acknowledged, progress-edited, finally rendered Telegram message. The gate for this item is verified: ratatoskr-extractor consumes `content.capture.requested.v1` end to end (JetStream consumer through outbox-published reports) and Platform serves authenticated capture submission plus a per-operation SSE progress stream.

Scope interpretation, stated so plan text cannot be misread: this change covers bare URLs in authorized private messages and the `/summarize <url>` form, one operation per capture, and truthful terminal rendering from what Platform actually exposes today. Batch/multi-URL, forwards and files (item 6), callback-token buttons (item 8), Mini App `initData` (item 9), and NATS transport (workspace integration) stay out.

Two scope items are named deferrals with evidence, not omissions:

- A cancel command cannot truthfully exist yet. Platform's served route table (`crates/public-api/src/lib.rs`) has no cancellation route, and no other Platform service exposes one; cancelling locally while the extractor keeps working would lie. `/cancel` waits for a Platform cancel capability (workspace changeset).
- Title/TLDR/key-points rendering is not possible yet. The operation snapshot carries status, stage, result references, errors, and warnings - never document text - and no Platform route returns blob content. Rendering invented summaries would violate the honesty rules. The completion render shows the truthful state plus links; rich result rendering waits for the analysis pipeline and a read surface.

## What Changes

- Message text in authorized private updates is parsed into typed intents: a bare http(s) URL and `/summarize <url>` become capture intents; anything else stays unsupported. Parsing validates scheme, host presence, and length before any external call.
- Capture submission derives its idempotency key deterministically per sender + normalized URL + intent kind, so resending the same link reuses the same Platform operation, while a deliberate retry after failure salts the key with the failed operation id and creates a new operation.
- A new `ratatoskr-telegram-platform-api` crate speaks authenticated HTTPS to Platform: submit captures with `Idempotency-Key`, read operations, consume the per-operation SSE event stream, and exchange short-lived Ed25519 identity assertions for sessions on the existing `POST /v1/sessions/telegram` route. Credentials are configuration secrets; sessions are cached per sender until near expiry.
- The webhook worker performs the first real domain action after access resolution: parse intent, obtain a session, submit the capture transactionally with the binding pre-created, enqueue the acknowledgment send job, settle the update. Platform failures retry briefly and then settle the update as failed with class-only telemetry.
- The dispatcher follows live operations: it watches non-terminal bindings, opens the Platform SSE stream per operation, maps frames onto the existing projection seam (event ids dedupe, statuses map onto the closed vocabulary), resumes with `Last-Event-ID`, and stops at terminal states. Restart recovery is the bindings table itself.
- Terminal renders gain content and buttons: completion renders the status line, safe detail lines, a fallback hyperlink to the captured article, and a Mini App deep-link button backed by an opaque server-side intent row in a new `telegram.interaction_intents` table; failure renders the failure state with actionable guidance instead of a retry button (retry-as-button needs callback tokens, item 8).
- The Bot API client learns `parse_mode=HTML` and inline-keyboard reply markup, and the outbound queue carries structured payloads so renders keep their markup end to end.

Out of scope, unchanged from the plan: file/PDF and forwarded-message ingestion (item 6), GitHub flows (item 7), callback tokens and dialogue state (item 8), Mini App initData validation (item 9), notifications (item 10).

## Capabilities

### New Capabilities

- `article-capture`: Turning an authorized private-message URL or `/summarize` command into an idempotent Platform capture operation, acknowledging it in a bound message, following it to a terminal render with links - including intent parsing rules, deterministic key derivation, assertion-authenticated submission, SSE follow behavior, and restart recovery.

### Modified Capabilities

- `operation-projection`: Terminal renders MAY compose a completion/failure body with safe detail lines, a fallback hyperlink, and a deep-link button resolved from a server-side intent; buttons appear only on terminal renders of bound operations.
- `outbound-delivery`: Send and edit jobs MAY carry a structured payload (HTML text plus inline keyboard) delivered verbatim through the Bot API sink; payload shape is internal, ordering and throttling requirements unchanged.
- `persistence-schema`: Adds the `telegram.interaction_intents` table (opaque intent records for Mini App deep links) and extends `telegram.outbound_jobs` to carry structured payloads.
- `service-configuration`: Adds the validated `RATATOSKR__PLATFORM__*` section (base URL, audience, timeout, assertion signing key) required by both runtime roles.
- `bot-api-client`: Adds HTML parse mode and inline-keyboard markup to the send/edit surface behind the existing error taxonomy.

## Impact

- New crate `crates/platform-api` (reqwest/rustls client, closed error taxonomy, assertion issuance and session cache); both services depend on it.
- `services/webhook/src/intake/worker.rs` gains the domain-action arm after `access::authorize`; startup threads the platform client into the worker.
- `services/dispatcher` gains an operation follower beside the projection consumer, fed by the bindings table; `build.rs` spawns it with the shared feed handle.
- `schema.sql` edits in place per development status: `telegram.interaction_intents` new table; `telegram.outbound_jobs` payload column added.
- `crates/bot-api`, `crates/core` config model/validation (new V-rules), `crates/persistence` repositories for intents and payload jobs, `crates/telemetry` class-only metrics.
- Cross-repository surface consumed, not changed: Platform `POST /v1/captures`, `GET /v1/operations/{id}`, `GET /v1/operations/{id}/events`, `POST /v1/sessions/telegram`; producer-side guarantees cited from the ratatoskr-workspace store's `operation-progress` spec, not restated here.
