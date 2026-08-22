# persistence-schema Specification

## Purpose
The `telegram` PostgreSQL schema this service owns, how it is applied at startup, and the readiness check that watches it.

## Requirements

### Requirement: The service owns one first-version schema

The service SHALL own the `telegram` PostgreSQL schema and no other. The schema definition SHALL be one file at the repository root; a schema change edits that file in place and there is no migration ledger and no second version.

#### Scenario: A fresh database receives the schema
- **WHEN** the schema file is applied to an empty database
- **THEN** the `telegram` schema exists

#### Scenario: The schema is defined in exactly one editable file
- **WHEN** a schema change is needed
- **THEN** it is made by editing the root `schema.sql` in place, and the deployed binary embeds the same file contents it was built with

### Requirement: Schema application is startup work, idempotent and all-or-nothing

When a database is configured, a process SHALL apply the schema at startup before reporting itself ready: application SHALL be skipped when the schema already exists, SHALL be safe to run concurrently with another starting process, and SHALL either apply completely or leave the database unchanged.

#### Scenario: Applying twice changes nothing the second time
- **WHEN** the schema is applied to a database that already has it
- **THEN** the second application makes no changes and reports success

### Requirement: The database readiness check probes in the background

When a database is configured, the process SHALL probe it on a fixed interval in the background and expose the latest result through `/health/ready`; when no database is configured there SHALL be no database check.

#### Scenario: An answering database turns the check green
- **WHEN** a configured database accepts queries
- **THEN** the next background probe marks the database check passing
