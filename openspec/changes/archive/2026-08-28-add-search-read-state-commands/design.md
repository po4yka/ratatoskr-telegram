## Context

See `proposal.md` for motivation. Authorized updates are claimed and processed after the webhook has acknowledged Telegram. The capture and settings flows show the command, Platform-session, durable outbound, and safe HTML patterns. The interaction-token registry currently supports callback and deep-link surfaces with 64-character tokens and complete scope checks; `interaction_tokens` has action-specific constraints and cleanup. Platform is the only allowed client boundary for search/read state.

## Goals / Non-Goals

**Goals:**

- Add a small deterministic command family without changing webhook latency or dispatcher ordering.
- Keep target identifiers server-side and prevent forwarded/copied read authority.
- Preserve honest outcomes under Platform absence, timeout, or uncertain response.
- Store no library projection or history beyond unavoidable bounded outbound payload retention.

**Non-Goals:**

- Search mode selection, multi-page dialogue, rich reader content, or group-chat results.
- Mark-unread, bulk mutation, favorites, saved searches, or a local cache of Knowledge data.
- A callback button variant; the first slice makes the named `/read` command testable directly.

## Decisions

### D1: Command routing precedes generic unsupported/capture routing

A `library` intake module parses exact slash commands after access control but before bare URL/capture fallback. Invalid library-shaped commands are claimed and receive usage rather than falling through as unsupported. The worker performs Platform calls only after webhook admission/acknowledgement, preserving the existing latency contract.

### D2: Platform capability and query calls use the existing user session path

`crates/platform-api` adds strict types for capabilities, library pages, and PUT state. The same short-lived Telegram assertion/session cache used by capture authenticates these calls. Each command reads current capabilities rather than caching them across deployments. Timeouts and retry classes follow the existing client taxonomy; searches are not retried after a valid permanent response.

### D3: Telegram requests exactly five results and renders one message

The adapter fixes `limit=5`, `offset=0`. The renderer budgets title, snippet, command-token, separators, and whole message below 4096 characters, escapes every dynamic field as HTML, and omits absent match fields. It creates no pagination state. The outbound job is a direct interaction response and therefore uses the existing higher priority.

### D4: Extend the shared token table with command-surface read authority

The current schema definition adds surface `command`, action `library_read`, nullable `analysis_id`, and an action-scoped `internal_user_id`, with action-specific checks: a library-read row requires both identities and forbids operation/dialogue-specific payload. The stored scope includes bot, Telegram actor, internal user, and chat; message binding is absent because the command token is textual. Issuance and reply enqueue occur through one persistence transaction so a rendered command never references missing authority. Cleanup uses the existing expiration/consumption rules.

Reusing `operation_id` for analysis identity was rejected as misleading and unsafe under future constraints. Exposing the analysis UUID directly was rejected because forwarding and brute-force attempts would move authorization burden into command parsing.

### D5: Consume once, then perform bounded idempotent PUT

Concurrent `/read` presentations have one database winner. That worker calls Platform PUT and retries only retryable transport/server classes within the existing small bound. A final lost response cannot restore the token safely without creating a second authority state machine, so the reply states `outcome unknown` and directs reconciliation with `/unread`. This is safe because PUT itself is idempotent.

### D6: Telemetry is class-only

Metrics use the closed command names `search`, `unread`, `read` and outcomes such as `succeeded`, `invalid`, `unavailable`, `not_found`, `expired`, and `unknown`. Logs reuse correlation/update identifiers but omit query/result/target values. Tests install a recording subscriber and inspect labels/fields.

## Risks / Trade-offs

- [Textual `/read` tokens make replies longer] -> five-result cap and deterministic budgets guarantee one valid message; inline buttons can be a later presentation-only change.
- [Outbound storage temporarily contains snippets] -> only the escaped bounded reply is retained, private chats are required, and normal outbound retention removes it; no query/result table is added.
- [A user cannot retry an uncertain consumed token] -> `/unread` reconciles authoritative state and returns a fresh token only if still unread.
- [Capability lookup adds one request] -> the single-host loopback path is bounded and avoids making unavailable features appear functional; no cross-session cache can go stale.

## Migration Plan

Apply only after Platform's OpenAPI and capability names are available. Update `schema.sql` in place, create disposable test databases from it, deploy the webhook role before exposing help text, and validate with the workspace composed profile. Roll back Telegram first; existing rows are harmless and disappear with normal cleanup or database recreation during development.
