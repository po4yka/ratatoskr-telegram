## Purpose

Structured logs, trace correlation, span export and metrics for every Ratatoskr Telegram binary.

## ADDED Requirements

### Requirement: Intake metrics are bounded-vocabulary instruments

The metric registry SHALL own the intake instrument names: a request-outcome counter over the closed outcome vocabulary, a received-update counter whose kind label maps unknown kinds to `other`, and an admission-duration histogram on the shared buckets. No label value SHALL be derived from request content outside those closed sets.

#### Scenario: Intake counters appear in the exposition

- **WHEN** webhook requests of several outcome classes are served and `/metrics` is scraped
- **THEN** the outcome, update-kind and duration series are present with bounded label values
