## ADDED Requirements

### Requirement: The deployment guide documents the Platform assertion key procedure

`deploy/README.md` SHALL document generating the key once with `assertion-key generate`, installing it at `/etc/ratatoskr/secrets/telegram-platform-signing-key` (`root:ratatoskr-telegram-secrets 0640`), printing the public half with `assertion-key public`, and setting it as `RATATOSKR__IDENTITY__ASSERTION_KEY` in the Edge configuration file. Each example SHALL point at that section beside `RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY_FILE`.

#### Scenario: The procedure regresses

- **WHEN** structural deployment validation reads the guide
- **THEN** it fails if any of the two commands, the install path, the Edge variable or the Edge configuration file is not named

### Requirement: The PostgreSQL role both examples name is provisioned

`deploy/postgres/role.sql` SHALL create the role `ratatoskr_telegram` without a password (set separately) and without superuser, createdb, createrole, replication or bypassrls, grant it connect and create on database `ratatoskr`, and revoke its rights on schema `public`. Both examples SHALL name that role, that database and port 5432, and the guide SHALL state the order: Edge first, then `role.sql`, then the Telegram services.

#### Scenario: An example names a role the file does not create

- **WHEN** structural deployment validation compares the webhook and dispatcher database URLs with `role.sql`
- **THEN** it fails if the role, the database or the port differs
