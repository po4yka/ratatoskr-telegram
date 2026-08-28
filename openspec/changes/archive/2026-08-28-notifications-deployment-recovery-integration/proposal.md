## Why

The Telegram integration can process interactions and project operation results, but it cannot yet honor user notification policy for fleet-raised facts and has no production-shaped deployment or executable recovery story. Plan item 10 closes those owned boundaries without reviving legacy digest scheduling or domain/model administration.

## What Changes

- Extend the editable first-version `telegram` schema with per-user notification preferences, daily quiet hours, and durable notification inbox/deduplication state; no migration files or alternate schema versions are introduced.
- Consume the canonical `platform.notification.raised.v1` envelope from `evt.platform.notification.raised.v1`, resolve the recipient's authorized private chat, apply per-class toggles and quiet hours, and enqueue a privacy-minimized outbound message exactly once per notification identity.
- Preserve unknown well-formed notification classes without guessing a toggle; default them to the user's global notification policy and record the decision safely.
- Add structurally validated systemd units for webhook and dispatcher using the workspace allocations `8182`, `9467`, and `9468`, bounded resources, explicit start ordering, NVMe logs, secret files, and `TimeoutStopSec=130s`.
- Add executable runbooks and guarded tooling for webhook-secret rotation, bot-token rotation, Platform session/stuck-operation recovery, and dead-update/dead-outbound inspection. Dry-run/read-only modes must not mutate provider, database, or host state.
- Document and exercise the existing plan-item-5 article flow in the workspace TG-010 composed profile, plus one synthetic raised notification.
- Do not add a Telegram health/admin command: user `/status` remains operation status, while service health remains on the existing private operator plane.

## Capabilities

### New Capabilities

- `notification-delivery`: Durable notification consumption, recipient resolution, preferences, quiet-hours enforcement, deduplication, safe rendering, and outbound enqueueing.
- `deployment-profile`: Single-host systemd artifacts, resource/exposure constraints, startup ordering, logging, and structural validation for webhook and dispatcher.
- `operational-recovery`: Executable, fail-closed inspection and recovery procedures with dry-run evidence and explicit mutation boundaries.

### Modified Capabilities

- `persistence-schema`: Add current-schema notification preference and notification inbox/deduplication records.
- `service-configuration`: Add validated NATS notification-consumer and deployment-facing settings without scattered environment reads.
- `operator-plane`: Reflect required notification-consumer dependencies and recovery-relevant queue health truthfully without exposing a new public surface.
- `outbound-delivery`: Enqueue notification sends through the existing durable ordered/rate-limited sender with direct interaction responses retaining priority.
- `telemetry`: Record bounded notification accepted, suppressed, deferred, delivered, duplicate, and failed classes without user or content labels.

## Impact

- Affected code: `schema.sql`, persistence APIs, dispatcher consumer/composition/startup wiring, typed configuration, telemetry, synthetic tests, and deployment/runbook assets.
- New dependency edges: Telegram pins the existing `ratatoskr-notification-contracts` revision and adds the repository-standard NATS client needed to consume the documented event subject. No provider SDK, migration framework, scheduling engine, or mocking crate is added.
- External systems: PostgreSQL 17, the existing `ratatoskr_events` JetStream stream, Telegram Bot API outbound delivery, Platform operation/session inspection, and the workspace TG-010 profile.
- Compatibility is additive. Producers need not change; absence of notification events preserves current behavior. Rollback stops the durable consumer and restores the prior binaries while retaining dedupe and preference evidence.
- The frozen single-host target is not modified. Structural and composed evidence is distinct from live deployment, webhook registration, and real-chat delivery.
