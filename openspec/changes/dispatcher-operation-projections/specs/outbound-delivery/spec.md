## Purpose

Delivers Telegram Bot API writes on behalf of the service through one durable, ordered, rate-limited queue: every send and edit is a persisted job with an explicit lifecycle, per-chat FIFO ordering, bounded retries, and idempotent edits, so projections and later command flows can enqueue work and trust it to be delivered truthfully exactly as far as it got - across restarts included.

## ADDED Requirements

### Requirement: Every outbound Bot API write is a durable job

The dispatcher SHALL represent every `sendMessage` and `editMessageText` call as a row in `telegram.outbound_jobs` carrying the bot id, target chat, method kind, rendered text, a content hash of that text, the correlation ids of the cause, and its attempt state, before any network call is made. A job SHALL NOT be lost by process exit between acceptance and delivery.

#### Scenario: A job accepted before delivery survives a restart

- **WHEN** a job is durably recorded in `ready` state and the dispatcher process exits before calling the Bot API
- **THEN** a fresh dispatcher claims that same job from PostgreSQL and delivers it without any external trigger

### Requirement: Per-chat strict FIFO ordering with one job in flight

The sender SHALL claim jobs so that within one chat, jobs deliver strictly in insertion order, and at most one job per chat is in flight at any moment. The service SHALL make NO ordering promise across different chats beyond preventing starvation.

#### Scenario: Concurrent sends to one chat arrive in order

- **WHEN** three send jobs for chat A are enqueued concurrently while jobs for chats B and C are also pending
- **THEN** the fake Bot API receives A's messages in enqueue order, and no two jobs of chat A are ever in flight simultaneously

### Requirement: Global and per-chat rate limits are enforced locally

The sender SHALL gate every Bot API call behind both a configurable global calls-per-second budget and a configurable minimum interval per chat. When Telegram answers `429` with `retry_after`, the sender SHALL reschedule the affected job at `now + retry_after + jitter`, treat that delay as authoritative for the chat, and count the wait in telemetry.

#### Scenario: Rate limiter spaces calls under burst

- **WHEN** more jobs than the configured budget allows are claimed in one instant
- **THEN** the observed Bot API call times respect both the global budget and the per-chat minimum interval without dropping any job

#### Scenario: Retry-After is honored authoritatively

- **WHEN** the fake Bot API answers a send with 429 and `retry_after: 30`
- **THEN** the job re-runs only after approximately 30 seconds, and no further call for that chat happens earlier

### Requirement: Failure classification decides retry versus dead-letter

The sender SHALL classify Bot API outcomes into: success; success no-op (`message is not modified`); retryable transient (network failure, timeout); rate-limited (`retry_after`); and permanent (bot blocked, chat not found, membership lost, message cannot be edited, message-to-edit not found, invalid markup or payload, chat migrated). Transient failures SHALL retry with capped exponential backoff plus jitter up to a bounded attempt count, then dead-letter as `failed_permanent`. Permanent failures SHALL dead-letter immediately. A timeout after the request was sent SHALL be retried (at-least-once), accepting a bounded duplicate-message window, and this semantics SHALL hold uniformly for sends and edits.

#### Scenario: Network failure retries then dead-letters

- **WHEN** the fake Bot API refuses connections for every attempt against one job
- **THEN** the job is attempted exactly the configured bound of times with growing backoff and ends `failed_permanent`

#### Scenario: Blocked chat dead-letters immediately

- **WHEN** the Bot API answers `Forbidden: bot was blocked by the user`
- **THEN** the job settles `failed_permanent` after exactly one attempt and no retry is scheduled

#### Scenario: Message-not-modified is a successful no-op

- **WHEN** an edit job's Bot API answer is `Bad Request: message is not modified`
- **THEN** the job settles `sent`, the binding's last rendered revision advances, and nothing is retried

### Requirement: An edit is applied at most once per logical revision

Before delivering an edit job the sender SHALL compare the job's projection revision against the binding's last rendered revision and skip (mark `superseded`) any job whose revision is not strictly newer. The comparison SHALL happen transactionally at claim time so a newer job enqueued while an older one is in flight wins. Identical rendered content (same hash) SHALL be skipped without an API call.

#### Scenario: Stale edit never reaches the wire

- **WHEN** revision 4 and revision 5 edits exist for one binding and revision 5 is delivered first
- **THEN** the stale revision 4 job is marked `superseded` and the Bot API never receives it

### Requirement: Delivery outcome updates the binding only after acknowledgment

A send job that carries an operation reference SHALL create or update its message binding with the returned Telegram message id only after the Bot API acknowledges success. Provider message ids SHALL never be recorded from unacknowledged attempts.

#### Scenario: Binding appears only after the ack

- **WHEN** a send job succeeds against the fake Bot API returning message id 42
- **THEN** the binding maps the operation to chat/message 42; while the same job is still in flight no binding row exists

### Requirement: Restart recovers in-flight work deterministically

A job left `sending` by a crashed process SHALL become claimable again after a bounded lease period, and shutdown SHALL drain the in-flight job before stopping rather than abandoning it mid-call.

#### Scenario: Interrupted lease is reclaimed after restart

- **WHEN** a job sits in `sending` with a stale lease timestamp and a new dispatcher starts
- **THEN** the new dispatcher reclaims and delivers it after the lease expires, without double-delivering any other job

## Purpose

Turns Platform operation snapshots into truthful Telegram progress messages bound to one chat message per operation: deduplicated consumption, monotonic revisions, throttled edits that never storm Telegram, terminal states rendered exactly once, and all dynamic text escaped.

## ADDED Requirements

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
