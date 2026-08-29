## Why

Startup currently treats the existence of the `telegram` namespace, or a short list of tables, as proof that the database matches the schema embedded in the running binary. After an in-place development schema edit, both roles can therefore start against stale storage and fail later in unrelated code paths.

## What Changes

- Record a deterministic fingerprint of the one current embedded schema when a fresh database is initialized.
- Make webhook schema application and dispatcher schema verification fail closed when the stored fingerprint is absent or differs from the running binary.
- Document recreation of a disposable development database as the recovery path; no migrations, schema versions, or compatibility routing are introduced.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `persistence-schema`: schema application and verification must establish an exact match with the current embedded definition rather than infer readiness from namespace or table existence.

## Impact

- `schema.sql` and `crates/persistence` schema startup paths and integration tests.
- Webhook and dispatcher startup will deliberately refuse a stale development database until it is recreated.
- No external API, dependency, migration, or production credential changes.
