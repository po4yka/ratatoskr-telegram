## Purpose

How a Ratatoskr Telegram process reads its configuration, validates it before binding anything, and refuses to start when it is wrong.

## ADDED Requirements

### Requirement: Configuration is read from the environment under one prefix

A process SHALL read all configuration from `RATATOSKR__` environment variables, with `__` separating nesting levels (for example `RATATOSKR__TELEMETRY__LOG_FORMAT`), over built-in per-role defaults. No code outside the configuration module SHALL read the process environment.

#### Scenario: A process starts in an empty environment
- **WHEN** a binary is started with no `RATATOSKR__` variables set
- **THEN** configuration loads from built-in defaults and the process binds its admin listener on that role's default loopback port

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
