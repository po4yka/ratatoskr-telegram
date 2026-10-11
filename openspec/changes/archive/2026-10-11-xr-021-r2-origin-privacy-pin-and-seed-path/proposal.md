## Why

XR-021 round 2 (CONTRACTS.md sections R2-17 and R2-10) records three defects that independent verifiers found by tracing the pushed round 1 code. A forwarded PDF sends the attachment's owner service, digest, media type and byte length to Platform inside `origin`, against the code's own comment that attachment metadata stays Telegram-local. Telegram pins the four `ratatoskr-*` contracts crates at `9a4df8126b495ffc3ad0647441da1690594f25bc` while every other repository pins `ad16855c4e7f3d52cd118274faa3b8f3ab4da576`, and Cargo treats two revisions of one git repository as different crates. The dispatcher example installs its NATS seed under `/etc/ratatoskr/secrets/` while Platform's NATS README and CONTRACTS S03 install every seed at `/etc/ratatoskr/<identity>.nkey`.

## What Changes

- The `origin` member of a capture submission carries the forward provenance only. Attachment facts never leave Telegram's own intent persistence, and a capture without a forward sends no `origin`.
- All four `ratatoskr-*` git dependencies move to `ad16855c4e7f3d52cd118274faa3b8f3ab4da576` and `Cargo.lock` is refreshed. No wire shape changes.
- `deploy/systemd/dispatcher.conf.example` and `deploy/README.md` install the dispatcher's NATS seed at `/etc/ratatoskr/telegram.nkey`, owned `root:ratatoskr-telegram-dispatcher` mode `0640`. The bot token and the assertion signing key keep their `/etc/ratatoskr/secrets/` paths.

## Capabilities

### Modified Capabilities

- `attachment-ingestion`: the Platform `origin` member is the forward member only.
- `deployment-profile`: the dispatcher example and guide install the NATS seed where the fleet does.

## Impact

- Hard break, no shim: an operator who installed the seed at `/etc/ratatoskr/secrets/telegram.nkey` moves it to `/etc/ratatoskr/telegram.nkey` and updates the dispatcher configuration. Platform ignores `origin`, so the narrowed member changes no Platform behaviour.
- Cross-repository behaviour is defined in XR-021 CONTRACTS.md sections R2-17 and R2-10 (rollout in S14), not in this change; this change cites those sections.
- `schema.sql` is untouched and there is no migration.
