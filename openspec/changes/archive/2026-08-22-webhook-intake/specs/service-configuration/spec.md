## Purpose

How a Ratatoskr Telegram process reads its configuration, validates it before binding anything, and refuses to start when it is wrong.

## MODIFIED Requirements

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

## ADDED Requirements

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
