## MODIFIED Requirements

### Requirement: Platform endpoint configuration

A process SHALL read the Platform base URL (`RATATOSKR__PLATFORM__BASE_URL`, default `http://127.0.0.1:8080`, the allocated Edge listener), call timeout in seconds (1..=60, default 10), token audience (`RATATOSKR__PLATFORM__AUDIENCE`, 1..=128 characters, default `edge`; an explicitly empty value is refused), and the Ed25519 assertion signing key (`RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY`, a secret encoding a 32-byte key) from one table. The base URL SHALL be https unless its host is a loopback address, mirroring the Bot API rule.

#### Scenario: A plain-http non-loopback platform endpoint is refused

- **WHEN** `RATATOSKR__PLATFORM__BASE_URL` is an `http://` URL with a public host
- **THEN** the process exits 78 and the violation names the key without echoing the value

#### Scenario: A malformed signing key is refused without echoing it

- **WHEN** `RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY` does not decode to 32 bytes
- **THEN** the process exits 78, the report names the key, and the supplied value appears nowhere

#### Scenario: A loopback harness endpoint is accepted

- **WHEN** the base URL is `http://127.0.0.1:<port>` for local testing
- **THEN** configuration validates

#### Scenario: Defaults target the allocated Edge

- **WHEN** the Platform section is loaded with no overrides
- **THEN** the base URL is `http://127.0.0.1:8080` and the audience is `edge`

#### Scenario: An empty audience is still refused

- **WHEN** `RATATOSKR__PLATFORM__AUDIENCE` is set to the empty string
- **THEN** the process exits 78 naming the key

### Requirement: Webhook listener configuration

The webhook role SHALL read its public listener bind address (`RATATOSKR__WEBHOOK__BIND`, default `127.0.0.1:8182`), webhook secret (`RATATOSKR__WEBHOOK__SECRET_TOKEN`, a secret), and maximum request body size (`RATATOSKR__WEBHOOK__MAX_BODY_BYTES`, 1024..=1048576, default 262144) from one table. The secret SHALL be 16..=256 characters over `[A-Za-z0-9_-]`. The public bind SHALL differ from the admin bind.

#### Scenario: A short secret is refused without echoing it

- **WHEN** `RATATOSKR__WEBHOOK__SECRET_TOKEN` is shorter than 16 characters
- **THEN** the process exits 78, the report names the key, and the supplied value appears nowhere

#### Scenario: Identical admin and public binds are refused

- **WHEN** both listeners are configured on the same address
- **THEN** the process exits 78 naming both keys

#### Scenario: The documented default matches the compiled default

- **WHEN** `.env.example` documents `RATATOSKR__WEBHOOK__BIND`
- **THEN** it documents `127.0.0.1:8182`, the compiled default, and names no other service's port
