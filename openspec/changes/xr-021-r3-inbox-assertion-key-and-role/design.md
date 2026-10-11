## Context

XR-021 CONTRACTS.md sections R3-01, R3-11 and R3-12 are the single source of truth. This document records only decisions internal to this repository.

## Decisions

### The connection options live in one function the test calls

`open_preprovisioned_consumer_at` already builds the connection; the inbox prefix is applied there with `custom_inbox_prefix(INBOX_PREFIX)`. The new test calls that function (exposed `pub` for the integration test, as `open_preprovisioned_consumer` already is) against a broker that runs only the Telegram stanza, so the proof goes through the real connection code instead of a re-implementation of its options.

### The authorized-broker test spawns `nats-server` with only the Telegram user

The test writes a server configuration that includes `deploy/nats/identity.conf` as the sole entry of `authorization.users`, with the nkey placeholder replaced by the public key derived from a seed generated in the test, plus a second admin user for fixtures (the role Edge plays). The admin creates `ratatoskr_events` and the durable the way Edge does. `nats-server` must be on `PATH` (CI copies it out of the pinned container; a developer installs it); a missing binary fails the test.

### One seed decoder, in `platform_api`

Both `decode_seed` copies validate 64 hex digits. They move to `platform_api::assertion_key::decode_seed`, which the two service builds call and the new subcommand reuses, so the three consumers cannot disagree on the format. The decoder never echoes the input in its error.

### The subcommand arm stays small

`services/webhook/src/main.rs` dispatches `assertion-key` before `telegram_http::run`, like `check-config`. The command logic lives in the library (`platform_api::assertion_key::{generate, public_key_base64}`) so it is unit tested; `main` only maps arguments to calls and exit codes. The dispatcher binary has no subcommand: the key is generated once, with the webhook binary, and shared through the secrets group.

### Provisioning and procedure are asserted from the shipped files

`deployment_profile.rs` reads `deploy/postgres/role.sql`, both examples and `deploy/README.md`, so the role, database, port and the documented commands cannot drift.

## Risks

- A seed generated on a host is the only copy; losing it means generating a new one and updating Edge's `RATATOSKR__IDENTITY__ASSERTION_KEY`. The procedure says so.
