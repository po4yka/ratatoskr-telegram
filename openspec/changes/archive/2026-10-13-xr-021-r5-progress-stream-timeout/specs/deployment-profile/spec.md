## ADDED Requirements

### Requirement: The documentation states which timeout bounds which call

`README.md` and `deploy/systemd/dispatcher.conf.example` SHALL state that `RATATOSKR__PLATFORM__TIMEOUT_SECONDS` bounds Platform request/response calls and the stream connect, not the progress stream, and that a progress stream with no byte for 45 seconds ends.

#### Scenario: The statement regresses

- **WHEN** a reader compares the README and the dispatcher example with the client
- **THEN** both name the 45 second idle bound and the call-timeout scope
