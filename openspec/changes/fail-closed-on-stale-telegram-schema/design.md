## Context

Database schema application currently serializes startup but skips all work when the Telegram namespace exists. Verification checks only five relations. Neither proves that the namespace matches the root schema definition embedded by this build. Development status forbids migrations and expects database recreation after an in-place definition change.

## Goals / Non-Goals

**Goals:**

- Establish an exact, deterministic match between a running binary and its owned schema before readiness.
- Preserve concurrent, all-or-nothing fresh initialization.
- Produce a safe actionable startup error without leaking connection details.

**Non-Goals:**

- Migrating or repairing an existing schema.
- Supporting more than one schema version or mixed-version rollout.

## Decisions

### D1: Hash the exact embedded schema bytes

A SHA-256 digest of the embedded schema bytes becomes the definition identity. Fresh application creates a singleton authority row and writes that digest in the same transaction as every schema object. The digest is derived from the embedded source rather than maintained manually, so changing the schema cannot omit a separate version bump.

A hand-maintained integer version was rejected because it recreates a migration/version ledger and can drift from the actual definition.

### D2: Existing namespace requires matching authority

Under the existing advisory lock, both application and verification read the singleton digest. Missing authority or a mismatch returns a typed safe schema error and performs no DDL. A fresh database is the only path that executes the schema definition.

Comparing selected catalog objects was rejected because it cannot prove constraints, functions, indexes, or future objects match.

### D3: Recreate is the only recovery

Development documentation will name database recreation as the recovery action. Startup will not add the authority row to an existing namespace because that would bless unknown contents without proof.

## Risks / Trade-offs

- [Any formatting-only schema edit changes the digest] -> Accept this conservative failure because exact source identity is cheap in the disposable development phase.
- [Old databases stop booting immediately] -> Return a distinct safe error and document the exact recreate workflow.
- [Concurrent first startups race] -> Keep the existing transaction-scoped advisory lock and write schema plus digest in one transaction.

## Migration Plan

There is no migration. Recreate each development database from the upgraded binary's embedded current schema. Rollback likewise requires recreation from the selected binary because mixed schema definitions are unsupported.
