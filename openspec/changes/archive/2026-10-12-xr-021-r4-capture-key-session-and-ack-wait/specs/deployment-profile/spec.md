## ADDED Requirements

### Requirement: The dispatcher example states why the notification ack wait is 30

`deploy/systemd/dispatcher.conf.example` SHALL set `RATATOSKR__NOTIFICATION_BUS__ACK_WAIT_SECONDS=30` with a comment that Edge fixes the durable at 30 seconds and that any other value stops the dispatcher at startup, and `deploy/README.md` SHALL mention the value where it lists the dispatcher keys.

#### Scenario: The comment regresses

- **WHEN** structural deployment validation reads the example and the guide
- **THEN** it fails if the value is not 30 or the explanation is missing
