## MODIFIED Requirements

### Requirement: Units enforce the single-host safety envelope

Each unit SHALL use `Type=exec`, `TimeoutStopSec=130s`, bounded restart/start-limit settings, explicit resource ceilings, least-privilege service identity, no-new-privileges and filesystem hardening, NVMe-backed append-only service logs with rotation, and explicit dependency ordering. Secrets SHALL be loaded from root-owned credential files and SHALL NOT appear in unit files, checked-in environment examples, command output, or logs. Unit network controls SHALL preserve outbound reachability to the configured Bot API and other required HTTPS dependencies; listener exposure SHALL remain bounded by the declared bind addresses, trusted ingress path, and target-host firewall policy.

#### Scenario: Required safety directives are absent

- **WHEN** a unit omits or weakens a required supervision, resource, logging, credential, or hardening directive
- **THEN** structural validation fails naming the missing property

#### Scenario: Unit network policy blocks the configured Bot API

- **WHEN** structural deployment validation examines a unit whose address policy denies public HTTPS egress required by the production Bot API endpoint
- **THEN** validation fails and identifies that the runtime dependency is unreachable under the shipped unit policy

#### Scenario: Public egress is enabled without publishing listeners

- **WHEN** structural deployment validation examines both corrected runtime units
- **THEN** the configured HTTPS dependencies remain reachable while the webhook, operator, and dispatcher listener boundaries remain unchanged
