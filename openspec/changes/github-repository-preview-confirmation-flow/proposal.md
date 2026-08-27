## Why

A pasted GitHub repository URL currently falls through the generic article-capture grammar, while the required repository workflow has no preview, confirmation, or truthful partial-result projection. Telegram must route this input to GitHub's live read surface and ensure no catalog, backup, or provider write fires before the owning user consumes a matching opaque confirmation token.

## What Changes

- Recognize only canonical GitHub repository URLs in ordinary messages before generic article routing and request a preview through the existing authenticated Platform client at `/v1/gh`.
- Render an escaped preview card with repository name, description, star count, primary language, and available `metadata`, `track`, and `star` buttons.
- Add a minimal owner/chat/bot-bound callback-intent store with high-entropy opaque tokens, expected repository/account state, mode, expiry, one-time transactional consumption, and stable action idempotency key.
- Make every mode a two-step flow: selecting a mode renders a confirmation prompt; only the prompt's confirmed token may submit the action. Stale, replayed, foreign, cancelled, or malformed callbacks submit nothing and are answered promptly.
- Render GitHub's component outcomes verbatim in meaning: metadata, provider star, and desired backup acceptance are separate, including failed, refused, skipped, already-applied, and accepted states. Do not infer backup completion or compensate provider mutations.
- Add a fake GitHub/Platform harness and RED-first acceptance tests for preview rendering, confirmation gating, callback replay/ownership, and partial-result rendering.
- Update the first-version schema in place, current docs, and the pinned contracts revision; star-list UI and OAuth remain out of scope.

## Capabilities

### New Capabilities

- `github-repository-flow`: GitHub URL routing, preview/confirmation UX, opaque callback intents, action submission, and truthful result projection.

### Modified Capabilities

- `article-capture`: A canonical GitHub repository URL routes to repository preview instead of content capture.
- `persistence-schema`: The current schema gains callback intent state and replay/ownership constraints without a migration.

## Impact

- Webhook intent routing/worker, persistence, outbound composition, Platform client, callback handling, synthetic Bot API/GitHub harnesses, schema, README/interfaces, and the pinned contracts revision.
- Affected surfaces: webhook handler, callback, dialogue-like confirmation state, Bot API callback acknowledgment, outbound preview/result projection, and configuration already used for Platform sessions. Mini App auth, dispatcher operation following, file handling, notifications, OAuth, and star-list selection do not change.
- `ratatoskr-contracts` and the live `ratatoskr-github` API must merge and pass their gates first. Telegram then validates against the live local GitHub service before integration.
