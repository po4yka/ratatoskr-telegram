## Why

Plan item 5 made a bare URL or `/summarize <url>` the only capture input. The bot's contract promises more: forwarded channel posts carrying links and document/photo attachments must reach Platform as captures too, and inputs the service cannot ingest (voice, video, audio transcription) deserve an explicit truthful reply instead of silence. This is implementation-plan item 6.

## What Changes

- Forwarded private messages that carry an external http(s) link (in text or caption) parse into the existing capture intent, with the forward origin preserved as bounded metadata on the intent record and carried additively in the submission body. Multiple links in one forward take the first; bundling stays out of scope.
- Document (`application/pdf`) and photo attachments within configured size limits are downloaded through the Bot API inside a byte budget, hashed with SHA-256 while streaming, and stored in a telegram-owned content-addressed blob store that follows the fleet `blob-references` convention (`ratatoskr-workspace` store spec). The submitted capture references the stored blob (a `BlobRef`: owner service, digest, media type, length) instead of a URL; nothing else about the capture flow changes - same idempotency discipline, binding, ack, projection follow.
- Attachment types outside the supported set (video, voice, audio, and other media) receive exactly one explicit HTML reply naming that the type is not supported yet, instead of being dropped silently. Transcription/TTS remains unowned fleet-wide and is not planned here.
- New typed configuration section `RATATOSKR__INGESTION__*` (attachment size budget), validated like every other key.
- The Bot API client gains file retrieval and a bounded streaming download; the token-carrying download URL never leaves that crate's boundary.
- Schema edits in place per development status: `telegram.interaction_intents.source_url` becomes nullable and a bounded `metadata jsonb` column carries forward-origin and blob-reference facts; no migration ledger.

**BREAKING** (development-status sense): the first-version schema changes shape in place; existing local databases must be recreated after this change lands.

## Capabilities

### New Capabilities

- `attachment-ingestion`: receiving document/photo attachments - allowlist and size gates before download, bounded Bot API streaming download, streaming SHA-256, the telegram-owned blob store and its `BlobRef` outputs, blob-referencing capture submission, and truthful unsupported-type replies.

### Modified Capabilities

- `article-capture`: forwards with links become capture intents with preserved provenance; captures may reference a stored blob instead of a URL; idempotency keys gain a digest-based derivation for blobs; terminal renders describe URL-less captures truthfully.
- `bot-api-client`: the client surface grows file retrieval plus a bounded streaming download exercised against local harnesses only.
- `service-configuration`: new `RATATOSKR__INGESTION__` keys with defaults and validation.
- `persistence-schema`: intents persist optional bounded metadata and a nullable source URL while keeping dedupe/expiry/ownership guarantees intact.

## Impact

- Crates: `bot-api` (file methods), `platform-api` (capture source + origin wire values), `core` (config + validation rule), `persistence` (intents shape), new `blob-store` module or crate following `extractor_blob_store`'s published pattern.
- Services: webhook intake worker (classification of forwards/attachments, download orchestration, explicit unsupported replies), dispatcher terminal composition (URL-less renders).
- Cross-repository note: Platform's public API accepts `{url}` captures only today, and the extractor consumes `{url}` commands only. Blob-source captures are modeled here and validated against local harnesses; until a workspace changeset extends Platform and the extractor, a real deployment settles blob captures as failed at the Platform call - truthfully surfaced through the existing projection failure path, never silently degraded to a fabricated URL.
