# deployment-profile Specification

## Purpose
Defines the production-shaped systemd profile and structural evidence for running Telegram's webhook and dispatcher roles on Ratatoskr's constrained single host.

## Requirements

### Requirement: One systemd unit owns each runtime role

The deployment SHALL provide separate webhook and dispatcher units using the release binaries, role-specific configuration, and the workspace port allocations: webhook public `8182`, webhook operator `9467`, and dispatcher operator `9468`. The dispatcher SHALL have no public listener.

#### Scenario: Unit files are validated structurally

- **WHEN** the deploy validation examines both unit files without starting a host service
- **THEN** each unit selects exactly one binary role and all listener values match the workspace allocations

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

### Requirement: Role startup reflects dependencies truthfully

The webhook SHALL start only after its configured database and public-listener prerequisites are available. The dispatcher SHALL start only after the current Telegram schema is ready and its database, Bot API credential, and notification bus credential are readable. Neither role SHALL claim readiness while a required dependency is unavailable.

#### Scenario: Dispatcher bus credential is unreadable

- **WHEN** the dispatcher starts with a configured but unreadable notification-consumer credential
- **THEN** it refuses startup or remains not ready with a safe dependency class and sends no notifications

### Requirement: Deployment validation does not mutate the target

Repository validation SHALL parse and inspect deploy artifacts without installing units, changing firewall or tunnel state, registering a webhook, rotating a secret, or contacting a real Telegram chat.

#### Scenario: Deploy gate runs on a development machine

- **WHEN** the structural deploy gate passes outside the target host
- **THEN** the evidence states deploy-artifact validity and does not claim live deployment

### Requirement: The shipped values target the allocated Edge

Both role configuration examples SHALL set `RATATOSKR__PLATFORM__AUDIENCE=edge` and `RATATOSKR__PLATFORM__BASE_URL=http://127.0.0.1:8080`, the audience Edge accepts and the port the workspace allocates to Edge. Neither example SHALL name `ratatoskr-platform`.

#### Scenario: An example regresses to the old values

- **WHEN** structural deployment validation examines the webhook and dispatcher examples
- **THEN** it fails if either lacks the exact audience and base URL lines or contains `ratatoskr-platform`

### Requirement: The attachment root is durable and readable by the extractor

The webhook example SHALL set `RATATOSKR__INGESTION__BLOB_ROOT=/mnt/nvme/ratatoskr/blobs/ratatoskr-telegram`. The webhook unit SHALL list that path in `ReadWritePaths` beside its existing paths, set `UMask=0027`, and include `ratatoskr-telegram-blobs` in `SupplementaryGroups`. The deployment guide SHALL create the group and the directory with `install -d` owner `ratatoskr-telegram-webhook`, group `ratatoskr-telegram-blobs`, mode `2750` (setgid so new files inherit the group), and SHALL state that the extractor reads the root read-only by BlobRef.

#### Scenario: The root regresses to the boot device

- **WHEN** structural deployment validation examines the example, the unit and the guide
- **THEN** it fails if the root, the `ReadWritePaths` entry, `UMask=0027`, the supplementary group or the `2750` install step is missing

### Requirement: The dispatcher installs its NATS seed where the fleet does

The dispatcher example SHALL set `RATATOSKR__NOTIFICATION_BUS__CREDENTIALS_FILE=/etc/ratatoskr/telegram.nkey`, and the deployment guide SHALL install that file with `install -m 0640 -o root -g ratatoskr-telegram-dispatcher`. The bot token and the assertion signing key SHALL keep their `/etc/ratatoskr/secrets/` paths.

#### Scenario: The example regresses to the secrets directory

- **WHEN** structural deployment validation examines the dispatcher example and the guide
- **THEN** it fails if the seed path is not `/etc/ratatoskr/telegram.nkey` in both, or if the guide lacks the `install -m 0640 -o root -g ratatoskr-telegram-dispatcher` command for it
