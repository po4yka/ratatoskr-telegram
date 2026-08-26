## ADDED Requirements

### Requirement: Send and edit payloads carry structured message content end to end

An outbound job SHALL carry its full rendered payload - HTML text and optional inline keyboard - as structured data that the delivery path passes to the Bot API verbatim, so parse mode and buttons survive queueing, restarts, and retries unchanged. The payload hash that suppresses identical re-renders SHALL cover the whole payload including markup. Jobs without markup SHALL keep their existing wire shape.

#### Scenario: Markup survives the queue to the wire

- **WHEN** a job enqueued with HTML text and an inline keyboard is delivered against the Bot API harness
- **THEN** the recorded request carries the same text under the HTML parse mode and the identical button layout

#### Scenario: A markup-less job stays byte-compatible

- **WHEN** a job carrying only text is delivered
- **THEN** the request omits markup fields exactly as before this capability existed
