# telemetry Specification

## Purpose
Structured logs, trace correlation, span export and metrics for every Ratatoskr Telegram binary.

## Requirements

### Requirement: Logs are structured and carry the request's trace context

A process SHALL write one log record per line to stdout in the configured format — JSON by default, human-readable pretty text for local runs. Every record emitted inside a traced operation SHALL carry that operation's `trace_id` and correlation fields.

#### Scenario: An invalid log filter is a configuration failure, not a telemetry crash
- **WHEN** `RATATOSKR__TELEMETRY__LOG_FILTER` is not a valid filter directive
- **THEN** the process refuses to start with a configuration error instead of failing inside subscriber setup

### Requirement: Trace ids exist without a collector

The process SHALL mint valid, non-zero W3C trace ids even when no OTLP endpoint is configured, so every log line carries a real `trace_id` on day one. When an OTLP endpoint is configured, completed spans SHALL be exported to it.

#### Scenario: No collector configured still yields real trace ids
- **WHEN** a process runs with no `RATATOSKR__TELEMETRY__OTLP` configuration
- **THEN** spans created inside it carry non-zero W3C trace ids and nothing is exported

#### Scenario: An HTTPS collector endpoint is exportable
- **WHEN** an OTLP endpoint with an `https` scheme is configured
- **THEN** the span exporter is constructed successfully (connection deferred until use)

### Requirement: Secrets never render

Configuration secrets SHALL be types whose debug rendering and error rendering redact them. A credential supplied as an OTLP header SHALL reach the exporter without appearing in any log line, `Debug` output or error message.

#### Scenario: A canary secret stays out of every rendering
- **WHEN** an OTLP header carries a marker value and telemetry initialises
- **THEN** the header reaches the exporter metadata, and the marker appears in neither the configuration debug output nor any telemetry error message
