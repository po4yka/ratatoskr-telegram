## Why

The bot currently processes every authenticated update: anyone who learns the bot username can submit work and consume resources. Before command flows arrive, interaction must be restricted to known users and chats, bootstrapped by a single owner taken from deployment configuration rather than a shared secret in the database.

## What Changes

- Add `telegram.identities` and `telegram.chats` tables that bind Telegram principals to internal users and carry per-principal access state (`enabled`/`disabled`). Private chats only; groups and supergroups remain out of scope.
- Bootstrap the owner from configuration: the webhook role demands an owner Telegram user id at validation, and on startup ensures an enabled owner identity exists for it.
- Authorize every processable update in the worker before any domain action: updates whose principal is unresolvable, unknown, disabled, or arriving from a non-private chat are settled as `denied` with no domain work and no reply.
- Extend the update state vocabulary with the terminal `denied` state; denied payloads are minimized at settlement like every other terminal state.

## Capabilities

### New Capabilities

- `identity-access-control`: which Telegram principals may interact with the bot, how the owner is bootstrapped, and how unauthorized updates are denied.

### Modified Capabilities

- `service-configuration`: a new access configuration table with the owner Telegram user id and its validation rule for the webhook role.
- `persistence-schema`: the identities/chats tables join the owned schema and the `denied` terminal state joins the update state vocabulary.
- `webhook-update-recovery`: terminal settlement additionally covers the `denied` state.

## Impact

- Schema: root `schema.sql` gains two tables; applied to fresh databases only, no migration ledger per development status.
- Code: `crates/persistence` gains a bindings repository and the denied transition; `crates/core/src/config` gains the access table and a validation rule; `services/webhook/src/intake/worker.rs` grows the authorization gate in the claim-and-settle path; unit and integration tests follow in the same crates.
- Deployment: one new webhook-required environment variable documented in `.env.example`; the variable must appear together with the upgraded binary because older builds reject unknown keys.
- Telemetry: denial outcomes counted without user identifiers in metric labels.
