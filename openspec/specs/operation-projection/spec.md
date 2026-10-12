# operation-projection Specification

## Purpose
Turns Platform operation snapshots into truthful Telegram progress messages bound to one chat message per operation: deduplicated consumption, monotonic revisions, throttled edits that never storm Telegram, terminal states rendered exactly once, and all dynamic text escaped.

## Requirements

### Requirement: Operation events are consumed deduplicated and order-tolerant

The consumer SHALL accept `platform.operation.progressed.v1` snapshots over a transport-independent seam whose contract is at-least-once delivery with no ordering guarantee, and SHALL drop an event whose envelope `event_id` was already consumed, recording the duplication. Subjects follow the contracts store's `evt.*` grammar; this service owns no producer side.

#### Scenario: Redelivered event changes nothing twice

- **WHEN** the same progressed snapshot arrives twice under one envelope event id
- **THEN** the second arrival produces no new outbound job and increments the duplicate counter

### Requirement: Guard precedence protects the bound message

For each event naming a bound operation the consumer SHALL apply guards in one transactional step, in this order: inbox deduplication; terminal-state check; staleness by envelope `occurred_at`; revision comparison. An event older than the newest already accepted for its binding SHALL be ignored without effect.

#### Scenario: Out-of-order progress cannot regress the render

- **WHEN** a running-stage snapshot arrives after a newer running snapshot for the same operation has been accepted
- **THEN** the older event is dropped by staleness and no edit job is created

### Requirement: Progress renders are throttled by durable rescheduling

The consumer SHALL enqueue every accepted non-terminal render as an edit job whose earliest attempt honors the binding's minimum render interval, computed from stored timestamps rather than in-process timers, so throttling survives restarts. Terminal transitions bypass the interval delay but not per-chat ordering.

#### Scenario: A progress burst does not storm Telegram

- **WHEN** ten progress ticks for one operation arrive within one second while the interval is four seconds
- **THEN** at most one intermediate edit becomes eligible per interval window and superseded intermediates never reach the Bot API

### Requirement: Terminal states render exactly once

The first event carrying a terminal status SHALL atomically set the binding's terminal flag and insert the terminal render job; any further terminal or post-terminal event for that binding SHALL be dropped with a class metric. A terminal projection SHALL never be overwritten by a later event.

#### Scenario: Duplicate completion events render once

- **WHEN** two `succeeded` snapshots for one operation arrive
- **THEN** exactly one terminal edit job exists across the queue's lifetime and the second event is counted as post-terminal

### Requirement: Renders escape untrusted text

Rendered progress text SHALL embed producer-supplied stage names, error and warning messages only through an HTML-escaping renderer, and SHALL branch displays on status, never on stage vocabulary.

#### Scenario: Hostile stage text renders inert

- **WHEN** a snapshot carries stage `<b>bold</b> & <script>alert(1)</script>`
- **THEN** the queued message body contains escaped entities and no executable markup

### Requirement: Unbound operations do not auto-create messages

The consumer SHALL edit only messages that an existing binding points at; an operation with no binding SHALL produce no outbound traffic. Send jobs that carry an operation reference MAY establish bindings upon acknowledged sends; bare progress events MUST NOT.

#### Scenario: Progress for an unknown operation stays silent

- **WHEN** a progressed snapshot arrives for an operation id with no binding row
- **THEN** no job is enqueued and the observation is counted, silently

### Requirement: Terminal renders compose links from server-side state

A terminal render for a bound operation MAY compose, beside the status-led escaped body, a fallback hyperlink to the operation's source address and one URL button whose target is the Mini App deep link carrying an opaque intent identifier resolved from this service's own intent records. Non-terminal renders SHALL remain plain status-led bodies without buttons or hyperlinks beyond those the escaping renderer already permits.

#### Scenario: Buttons ride only on the terminal render

- **WHEN** an operation's binding receives a running frame followed by its succeeded terminal
- **THEN** the running edit job carries no reply markup while the single terminal job carries the deep-link button and fallback hyperlink

#### Scenario: Markup-only terminal changes still edit

- **WHEN** a terminal render's composed body equals the previous render but adds markup
- **THEN** the job is not suppressed as identical content, because the payload hash covers markup too

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
