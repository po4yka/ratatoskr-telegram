## ADDED Requirements

### Requirement: Ingestion limits are typed configuration with defaults and validation

The configuration SHALL carry an ingestion section holding the attachment byte budget used both to refuse oversized declared sizes before download and to abort streaming downloads past the budget. The section SHALL follow the same environment mapping, unknown-field refusal, and violation reporting as every other section; its budget SHALL be bounded above by the Bot API's own download ceiling for bots, and a missing value SHALL default to a documented size within that ceiling.

#### Scenario: The budget parses from the environment and defaults sensibly

- **WHEN** the webhook role loads without an ingestion budget set
- **THEN** the budget equals the documented default, and setting `RATATOSKR__INGESTION__MAX_ATTACHMENT_BYTES` to any positive value at or under the Bot API ceiling loads exactly that value

#### Scenario: An out-of-range budget is refused with a named rule

- **WHEN** the budget is zero, negative in meaning, or above the Bot API ceiling
- **THEN** configuration loading fails naming the key, the environment variable, and the violated bound without quoting the offending value
