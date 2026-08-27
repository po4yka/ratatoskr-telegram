## Context

Item 5's pipeline is the extension point: `worker.rs::message_parts` extracts text/sender/chat, `intent::parse` recognizes URL text, `capture::submit` exchanges a session, POSTs `/v1/captures` with a deterministic idempotency key, then writes binding + intent + one ack job. The Bot API client has no file methods; `BotApiError::Io` was reserved for file work. teloxide's `Message` already carries `forward_origin`, `document`, `photo`, and `caption`. The fleet store spec `blob-references` (`openspec show blob-references --store ratatoskr-workspace`) fixes what a BlobRef is and forbids any blob storage service: each service writes its own content-addressed store; consumers resolve references without an API. Platform accepts `{url}` captures only today, and the extractor decodes `{url}` commands only.

## Goals / Non-Goals

Goals: one classification path where forwards and attachments join the existing capture flow; bytes that never exceed budget or memory bounds; durable provenance with no raw content; tests entirely on local harnesses.

Non-Goals: transcription/TTS (unowned fleet-wide), multi-link bundling (follow-up item), media conversion, Platform/extractor contract changes (named as cross-repo follow-up in the proposal), Mini App surfaces.

## Decisions

### D1: Telegram owns its own blob store, following the extractor's published pattern

The parity wording "extractor-owned blob store" cannot be implemented literally: the fleet spec forbids writing into another service's store and extractor exposes no upload route. Instead a new `crates/blob-store` mirrors `ratatoskr-extractor-blob-store`'s convention - stream chunks through SHA-256 into `<root>/staging/<uuid>.part`, then hard-link to `<root>/sha256/<2>/<62>`, verify owner/algorithm/digest/media-type/length - publishing `owner_service = "ratatoskr-telegram"` BlobRefs. Alternative rejected: an ingestion endpoint on extractor (needs a workspace changeset plus a spec amendment; out of this repository's authority).

### D2: Reuse `ratatoskr_identifiers` for BlobRef types

Git dependency pinned to the same contracts revision the extractor pins (`d56c6891...`, satisfying `deny.toml`'s `required-git-spec = "rev"`). The JSON schema fixtures in ratatoskr-contracts stay the compatibility authority. Alternative rejected: re-declaring local types (drift risk against a published contract).

### D3: Downloads are Bot API getFile plus a reqwest byte stream inside the bot-api crate

teloxide resolves `file_path`; the download itself uses the crate's existing reqwest stack so the token-bearing URL never leaves the boundary. The byte budget is enforced by the reader: copy chunks to the staging sink, abort once cumulative bytes pass the budget - declared size is advisory, the counter is authoritative. Memory stays O(chunk). The whole-call timeout still applies per HTTP call; streaming reads inherit it, which is acceptable at the ≤20 MiB Bot API ceiling.

### D4: Classification extends `message_parts`, not a parallel path

`MessageParts` grows origin/attachment fields; `self_domain_action` branches: forward-or-text link → URL capture (+origin); supported attachment → download/store/blob capture; unsupported media → one truthful reply job; else existing behavior. One settle path per update remains.

### D5: Provenance is bounded typed metadata in one jsonb column

`interaction_intents.metadata jsonb NULL` holds a strictly-typed serde value with `deny_unknown_fields`: optional forward-origin facts (kind vocabulary user/hidden_user/chat/channel, identifiers, original date) and optional blob facts (BlobRef fields, media type). CHECK constrains it to JSON objects and requires source_url IS NULL exactly when blob facts exist. Alternatives rejected: wide nullable columns (schema churn per new fact) and free-form jsonb (privacy boundary needs an explicit shape). Raw forwarded names are minimized to identifiers plus kind; no message text enters the column.

### D6: Wire shape is additive; real-Platform gap is documented, never papered over

URL captures keep posting exactly `{"url": ...}`. Blob captures post `{"blob": <BlobRef>, "media_type": ...}` plus optional `"origin"`; URL captures with provenance add `"origin"` additively (serde-tolerant servers ignore unknown members, so today's Platform keeps accepting provenance-carrying URL captures while ignoring them). A real Platform rejects blob sources as a missing-url client error → the capture settles failed through the existing bounded degradation path, visibly. Fabricating a URL or silently dropping is forbidden.

### D7: Idempotency keys hash the canonical source string

Blobs key on `"capture.v1|{user}|sha256:{hex}"` beside the existing URL form - same derivation family, same determinism guarantees.

### D8: Ingestion limits live under `RATATOSKR__INGESTION__MAX_ATTACHMENT_BYTES`

Default 18 MiB (under Bot API's 20 MiB ceiling so refusal happens before Telegram's own), validated as rule V18 via the existing `positive(...)` closure pattern with a named upper bound.

## Risks / Trade-offs

- [Sequential worker blocks on downloads] → Budget ceiling bounds worst case (~20 MiB over loopback-class links); single-owner deployment makes head-of-line blocking tolerable; revisit with workspace integration.
- [Platform cannot consume blob captures yet] → Truthful failure surfacing (D6); the cross-repo changeset is named in the proposal before any live reliance.
- [jsonb metadata could accrete content] → deny_unknown_fields shape + object CHECK + review duty; retention follows the intent row's expiry.
- [Hard links across filesystems fail] → Fall back to rename within the store root; both operations stay inside one configured directory tree.

## Migration Plan

Development status: edit `schema.sql` in place; drop and recreate local databases after pulling (startup applies schema only when absent). No data survives; nothing to roll back beyond reverting the commit.

## Open Questions

None deferred: the Platform-side contract extension is explicitly out of scope here and tracked by the proposal's Impact note.
