## ADDED Requirements

### Requirement: The progress stream is bounded by an idle timeout, not by the whole-call timeout

The Platform client SHALL follow an operation's progress over a client that has a connect timeout equal to the configured call timeout, a read timeout of `PLATFORM_STREAM_IDLE_TIMEOUT` (45 seconds, three Platform keep-alive intervals) and no total timeout. A stream that keeps receiving bytes, including keep-alive comments, SHALL NOT be ended by `platform.timeout_seconds`. A stream that receives no byte for the idle bound SHALL end as a clean close. Request/response calls SHALL keep the whole-call timeout. A resumed stream SHALL still send `Last-Event-ID`.

#### Scenario: A long-lived stream outlives the call timeout

- **WHEN** Platform keeps the stream open with keep-alives for longer than the call timeout and then sends a terminal frame
- **THEN** the client delivers the terminal frame over one connection and `next_frame` never reports a timeout

#### Scenario: A silent stream ends

- **WHEN** Platform sends the response head and then nothing for longer than the idle timeout
- **THEN** the stream ends instead of waiting for the whole-call timeout or forever

#### Scenario: A stalled request is still cut

- **WHEN** a request/response call receives a body that stalls past the call timeout
- **THEN** the call fails with a timeout

### Requirement: The follower follows a slow operation over one connection

The dispatcher's follower SHALL follow an operation that stays queued for longer than the call timeout over a single stream, with no reconnect and no `follow_frame_failed` log, until its terminal frame. The three-closes bound and the scan restart are unchanged.

#### Scenario: A queued operation is followed without reconnects

- **WHEN** an operation stays queued for several multiples of the call timeout before it finishes
- **THEN** the fake Platform sees exactly the requests of one follow
