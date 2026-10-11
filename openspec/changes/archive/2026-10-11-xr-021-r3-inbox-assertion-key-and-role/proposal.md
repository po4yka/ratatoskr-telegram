## Why

XR-021 round 3 (CONTRACTS.md sections R3-01, R3-12 and R3-11) records three defects that the round 2 native smokes and the independent verifiers found and that were re-verified against `origin/main`. The dispatcher connects to NATS with the default inbox prefix, so under Platform's per-service ACL (`subscribe: { allow: ["_INBOX_telegram.>"] }`) its JetStream replies are never delivered and every request times out; the repository also has no copy of its stanza and no test against a broker that enforces it. The Platform assertion signing key is a 64-hex Ed25519 seed that Telegram reads, while Edge wants the base64 public key, and the only recipe is an `openssl` plus `xxd` snippet in two workspace scripts that LibreSSL does not run reliably. Both examples name the role `ratatoskr_telegram` on the database `ratatoskr`, but nothing creates that role.

## What Changes

- The dispatcher's notification connection sets `custom_inbox_prefix("_INBOX_telegram")`, held as one `const`. `deploy/nats/identity.conf` is a byte-equal copy of the `TELEGRAM` stanza of Platform's `ratatoskr.conf`, and CI puts `nats-server` on `PATH` so a test can start a broker that enforces it.
- The webhook binary gains `assertion-key generate <path>` (creates a mode 0600 seed file exclusively, prints the public key) and `assertion-key public <path>` (prints the base64 public key). One seed decoder in `crates/platform-api` replaces the two private copies in the webhook and dispatcher.
- `deploy/README.md` gains a "Platform assertion key" section and the examples point at it.
- `deploy/postgres/role.sql` creates the `ratatoskr_telegram` role on `ratatoskr`, and `deploy/README.md` states the provisioning order.

## Capabilities

### New Capabilities

- `assertion-key`: generating and deriving the public half of the Platform assertion signing key.

### Modified Capabilities

- `notification-delivery`: the dispatcher connection uses its own inbox prefix.
- `deployment-profile`: the guide documents the assertion key procedure and ships the role provisioning file.

## Impact

- Hard break, no shim: a dispatcher built before this change cannot complete JetStream requests under Platform's new ACL (the reply is never delivered and the call times out). Nothing is deployed, so the break is named and not staged: reload NATS and deploy the services of one snapshot together.
- `RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY_FILE` keeps its format; the two private `decode_seed` functions are replaced by one public function in `platform_api::assertion_key`.
- Cross-repository behaviour is defined in XR-021 CONTRACTS.md sections R3-01, R3-11 and R3-12, not in this change; this change cites those sections.
- `schema.sql` is untouched and there is no migration.
