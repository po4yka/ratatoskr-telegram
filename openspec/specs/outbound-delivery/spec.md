# outbound-delivery Specification

## Purpose
Delivers Telegram Bot API writes on behalf of the service through one durable, ordered, rate-limited queue: every send and edit is a persisted job with an explicit lifecycle, per-chat FIFO ordering, bounded retries, and idempotent edits, so projections and later command flows can enqueue work and trust it to be delivered truthfully exactly as far as it got - across restarts included.

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

### Requirement: Send and edit payloads carry structured message content end to end

An outbound job SHALL carry its full rendered payload - HTML text and optional inline keyboard - as structured data that the delivery path passes to the Bot API verbatim, so parse mode and buttons survive queueing, restarts, and retries unchanged. The payload hash that suppresses identical re-renders SHALL cover the whole payload including markup. Jobs without markup SHALL keep their existing wire shape.

#### Scenario: Markup survives the queue to the wire

- **WHEN** a job enqueued with HTML text and an inline keyboard is delivered against the Bot API harness
- **THEN** the recorded request carries the same text under the HTML parse mode and the identical button layout

#### Scenario: A markup-less job stays byte-compatible

- **WHEN** a job carrying only text is delivered
- **THEN** the request omits markup fields exactly as before this capability existed

### Requirement: Background notifications have explicit lower queue priority

Outbound jobs SHALL distinguish direct interaction responses from background notifications. Ready direct responses SHALL be selected before background notifications for the same chat without bypassing per-chat ordering, provider rate limits, or durability, and background work SHALL remain bounded so it cannot starve indefinitely.

#### Scenario: Direct response and notification are both ready

- **WHEN** one direct response and one background notification are ready for the same chat
- **THEN** the direct response is attempted first and the notification remains durably eligible

### Requirement: Deferred notifications become ready without duplication

A notification deferred by quiet hours SHALL not be claimable before its recorded release time. At or after that time it SHALL enter normal outbound ordering exactly once, and repeated scheduler/consumer wakeups SHALL NOT create another job.

#### Scenario: Quiet-hours boundary passes

- **WHEN** the stored release time arrives for a deferred notification
- **THEN** one existing job becomes claimable and no second outbound payload is inserted
