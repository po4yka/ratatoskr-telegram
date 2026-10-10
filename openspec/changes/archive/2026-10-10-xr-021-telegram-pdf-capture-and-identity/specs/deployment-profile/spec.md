## ADDED Requirements

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
