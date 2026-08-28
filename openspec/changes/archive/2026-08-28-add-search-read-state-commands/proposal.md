## Why

The bot reserves archive search but still cannot retrieve authoritative library state. Knowledge search and user-content state now exist, and the coordinated Platform contract can expose them without moving search or read-state ownership into Telegram.

## What Changes

- Add exact private-chat `/search <query>`, `/unread`, and `/read <opaque-token>` command handling after existing authorization and durable update admission.
- Query Platform synchronously from the post-ack worker with finite limits; `/search` returns ranked results and `/unread` returns recent unread results.
- Render at most one bounded page as escaped Telegram HTML with title, optional snippet/match indication, effective read state, and one opaque mark-read action per unread result.
- Persist 64-character owner/bot/chat-bound, expiring, single-purpose read tokens that reference an analysis server-side and reveal no analysis identifier or content in Telegram callback/command data.
- Apply `/read` idempotently through Platform, acknowledge success only after an authoritative response, and report unavailable, missing, expired/foreign, and uncertain outcomes truthfully.
- Gate `/search` and `/unread` on Platform's `library.search` capability, gate read-token issuance and `/read` on `library.read_state`, and produce stable unavailable behavior without a Knowledge-facing call when the required name is absent.
- Update help/README command documentation and add safe telemetry without query text, titles, snippets, Telegram IDs, or analysis identifiers as metric labels.
- Keep semantic-mode selection, pagination dialogue, saved search/history, favorites, mark-unread/bulk read, natural-language inference, groups/channels, digests, and Mini App reader UI outside this change.
- Conform to workspace change `add-library-search-read-state-contract` and consume Platform change `add-library-search-read-state-api`.

## Capabilities

### New Capabilities

- `library-commands`: Telegram parsing, rendering, opaque read authority, Platform delegation, failure semantics, and command telemetry for search/unread/read.

### Modified Capabilities

- `interaction-token-registry`: The closed token registry accepts a new owner/chat-bound library read action without weakening existing callback or deep-link authority.

## Impact

- `services/webhook/src/intake` gains the command adapter and tests; work remains downstream of the fast webhook acknowledgment.
- `crates/platform-api` gains typed capability, library search, and read-state methods against Platform only.
- `crates/persistence`, `schema.sql`, and cleanup tests gain the current-schema read action token kind; no searchable domain projection or search history is stored beyond the bounded rendered payload already required by the durable outbound queue.
- Existing durable outbound jobs carry the rendered replies; dispatcher ordering/rate limiting remains unchanged.
- Rollback removes command routing and token issuance first. Already consumed tokens remain audit/deduplication evidence until normal cleanup, and Knowledge read state remains authoritative.
