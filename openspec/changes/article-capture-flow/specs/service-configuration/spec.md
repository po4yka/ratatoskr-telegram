## ADDED Requirements

### Requirement: Platform endpoint configuration

A process SHALL read the Platform base URL (`RATATOSKR__PLATFORM__BASE_URL`, default `http://127.0.0.1:9463` for the development harness), call timeout in seconds (1..=60, default 10), token audience (`RATATOSKR__PLATFORM__AUDIENCE`, 1..=128 characters), and the Ed25519 assertion signing key (`RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY`, a secret encoding a 32-byte key) from one table. The base URL SHALL be https unless its host is a loopback address, mirroring the Bot API rule.

#### Scenario: A plain-http non-loopback platform endpoint is refused

- **WHEN** `RATATOSKR__PLATFORM__BASE_URL` is an `http://` URL with a public host
- **THEN** the process exits 78 and the violation names the key without echoing the value

#### Scenario: A malformed signing key is refused without echoing it

- **WHEN** `RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY` does not decode to 32 bytes
- **THEN** the process exits 78, the report names the key, and the supplied value appears nowhere

#### Scenario: A loopback harness endpoint is accepted

- **WHEN** the base URL is `http://127.0.0.1:<port>` for local testing
- **THEN** configuration validates

### Requirement: Both roles require the Platform section at this milestone

Because both runtime roles now perform Platform work - the webhook submits captures and the dispatcher follows operations - each role SHALL demand the Platform section's audience and signing key plus its endpoint table at validation; missing any SHALL be refused before anything binds, in the same report that names every other missing requirement.

#### Scenario: The webhook binary refuses unconfigured

- **WHEN** the webhook binary runs without the Platform section
- **THEN** it exits 78 and the report names the missing Platform keys beside any other missing requirements

#### Scenario: The dispatcher binary refuses unconfigured

- **WHEN** the dispatcher binary runs without the Platform section
- **THEN** it exits 78 and the report names the missing Platform keys
