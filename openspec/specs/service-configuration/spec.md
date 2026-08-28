# service-configuration Specification

## Purpose

How a Ratatoskr Telegram process reads its configuration, validates it before binding anything, and refuses to start when it is wrong.

## Requirements

### Requirement: Ingestion limits are typed configuration with defaults and validation

The configuration SHALL carry an ingestion section holding the attachment byte budget used both to refuse oversized declared sizes before download and to abort streaming downloads past the budget. The section SHALL follow the same environment mapping, unknown-field refusal, and violation reporting as every other section; its budget SHALL be bounded above by the Bot API's own download ceiling for bots, and a missing value SHALL default to a documented size within that ceiling.

#### Scenario: The budget parses from the environment and defaults sensibly

- **WHEN** the webhook role loads without an ingestion budget set
- **THEN** the budget equals the documented default, and setting `RATATOSKR__INGESTION__MAX_ATTACHMENT_BYTES` to any positive value at or under the Bot API ceiling loads exactly that value

#### Scenario: An out-of-range budget is refused with a named rule

- **WHEN** the budget is zero, negative in meaning, or above the Bot API ceiling
- **THEN** configuration loading fails naming the key, the environment variable, and the violated bound without quoting the offending value

### Requirement: Configuration is read from the environment under one prefix

A process SHALL read all configuration from `RATATOSKR__` environment variables, with `__` separating nesting levels (for example `RATATOSKR__TELEMETRY__LOG_FORMAT`), over built-in per-role defaults. No code outside the configuration module SHALL read the process environment. A role whose operation requires credentials or dependencies SHALL demand them at validation rather than carrying silent defaults for them.

#### Scenario: A process starts in an empty environment

- **WHEN** a binary whose role carries no intake requirements is started with no `RATATOSKR__` variables set
- **THEN** configuration loads from built-in defaults and the process binds its admin listener on that role's default loopback port

#### Scenario: A process with role requirements starts in an empty environment

- **WHEN** the webhook binary is started with no `RATATOSKR__` variables set
- **THEN** validation refuses with every missing requirement named before anything binds

#### Scenario: An environment variable overrides a default
- **WHEN** `RATATOSKR__ADMIN__BIND` is set to `127.0.0.1:9998`
- **THEN** the admin listener binds on port 9998 and no other setting changes

### Requirement: Unknown configuration fields are rejected

The configuration tree SHALL reject an unknown key instead of ignoring it.

#### Scenario: An unknown field fails the load
- **WHEN** the environment carries `RATATOSKR__NO_SUCH_FIELD=x`
- **THEN** configuration loading fails, the failure report names the unknown key, and nothing is bound

### Requirement: Invalid configuration is refused with every violation reported

A process SHALL refuse to start when the parsed configuration violates a startup rule, SHALL report every violation found rather than only the first, SHALL write the report to stderr before any subscriber exists, and SHALL exit with status 78 (`EX_CONFIG`). A violation report SHALL NOT quote supplied values, so a secret in the environment cannot reach the report.

#### Scenario: Two bad values produce two violations in one report
- **WHEN** the shutdown grace window and the OTLP endpoint scheme are both invalid
- **THEN** the process exits 78 and the report describes both problems

#### Scenario: A valid configuration exits the check cleanly
- **WHEN** `<binary> check-config` runs with only defaults in the environment
- **THEN** it exits 0 and prints the effective configuration to stderr with secret fields redacted

### Requirement: The role is fixed by the binary

The deployable role (`webhook`, `dispatcher`) SHALL be compiled into the binary and never read from configuration or the environment. Each role SHALL have a distinct default admin port so both binaries run on one machine unconfigured.

#### Scenario: The roles differ without configuration
- **WHEN** the webhook binary and the dispatcher binary are started side by side with no configuration
- **THEN** each reports its own role name and binds a different default admin port

### Requirement: Bot API endpoint configuration

A process SHALL read the Bot API base URL (`RATATOSKR__BOT_API__BASE_URL`, default `https://api.telegram.org`), call timeout in seconds (1..=60, default 10), and bot token (`RATATOSKR__BOT_API__TOKEN`, a secret) from one table. The base URL SHALL be https unless its host is a loopback address.

#### Scenario: A plain-http non-loopback endpoint is refused

- **WHEN** `RATATOSKR__BOT_API__BASE_URL` is an `http://` URL with a public host
- **THEN** the process exits 78 and the violation names the key without echoing the value

#### Scenario: A loopback harness endpoint is accepted

- **WHEN** the base URL is `http://127.0.0.1:<port>` for local testing
- **THEN** configuration validates

### Requirement: Webhook listener configuration

The webhook role SHALL read its public listener bind address (`RATATOSKR__WEBHOOK__BIND`, default loopback port 9469), webhook secret (`RATATOSKR__WEBHOOK__SECRET_TOKEN`, a secret), and maximum request body size (`RATATOSKR__WEBHOOK__MAX_BODY_BYTES`, 1024..=1048576, default 262144) from one table. The secret SHALL be 16..=256 characters over `[A-Za-z0-9_-]`. The public bind SHALL differ from the admin bind.

#### Scenario: A short secret is refused without echoing it

- **WHEN** `RATATOSKR__WEBHOOK__SECRET_TOKEN` is shorter than 16 characters
- **THEN** the process exits 78, the report names the key, and the supplied value appears nowhere

#### Scenario: Identical admin and public binds are refused

- **WHEN** both listeners are configured on the same address
- **THEN** the process exits 78 naming both keys

### Requirement: The webhook role requires its dependencies to be configured

For the webhook role, `bot_api.token`, `webhook.secret_token` and a database URL SHALL all be present; missing any SHALL be refused at validation before any listener binds or any network call is made. The dispatcher role SHALL NOT require them at this milestone.

#### Scenario: The webhook binary refuses to validate unconfigured

- **WHEN** the webhook binary runs with only default configuration
- **THEN** it exits 78 and the report names every missing requirement

#### Scenario: The dispatcher still starts unconfigured

- **WHEN** the dispatcher binary runs with only default configuration
- **THEN** it starts and serves its operator plane

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

### Requirement: Notification consumption uses typed finite configuration

The dispatcher configuration SHALL define the NATS endpoint, event stream, durable consumer, exact notification subject, fetch/ack limits, and credential-file path under the existing Ratatoskr environment prefix. Unknown fields, non-TLS remote endpoints outside the local deployment allowance, wildcard notification subjects, zero bounds, and conflicting inline/file credentials SHALL be refused before listeners bind.

#### Scenario: Dispatcher is configured for the canonical subject

- **WHEN** configuration names `ratatoskr_events`, durable `ratatoskr_telegram_notifications`, and `evt.platform.notification.raised.v1` with positive bounds and a readable credential file
- **THEN** validation accepts the notification-consumer section without exposing credential contents

#### Scenario: Subject is widened to all events

- **WHEN** the notification subject is configured as `evt.>`
- **THEN** validation refuses startup because Telegram requires only the canonical notification fact

### Requirement: Runtime secrets support atomic file rotation

Bot token, webhook secret, NATS credential, and Platform signing material SHALL be loadable from explicit credential files with one configured source per secret. Effective configuration and validation errors SHALL render only source metadata and safe failure classes, never file contents.

#### Scenario: Inline and file secret are both configured

- **WHEN** a secret has both an inline value and a credential-file path
- **THEN** configuration fails closed and names the conflicting settings without rendering either value
