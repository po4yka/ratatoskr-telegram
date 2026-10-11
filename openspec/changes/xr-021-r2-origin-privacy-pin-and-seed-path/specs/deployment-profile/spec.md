## ADDED Requirements

### Requirement: The dispatcher installs its NATS seed where the fleet does

The dispatcher example SHALL set `RATATOSKR__NOTIFICATION_BUS__CREDENTIALS_FILE=/etc/ratatoskr/telegram.nkey`, and the deployment guide SHALL install that file with `install -m 0640 -o root -g ratatoskr-telegram-dispatcher`. The bot token and the assertion signing key SHALL keep their `/etc/ratatoskr/secrets/` paths.

#### Scenario: The example regresses to the secrets directory

- **WHEN** structural deployment validation examines the dispatcher example and the guide
- **THEN** it fails if the seed path is not `/etc/ratatoskr/telegram.nkey` in both, or if the guide lacks the `install -m 0640 -o root -g ratatoskr-telegram-dispatcher` command for it
