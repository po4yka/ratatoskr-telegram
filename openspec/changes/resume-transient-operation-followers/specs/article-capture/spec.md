## MODIFIED Requirements

### Requirement: Live operations are followed over Platform's event stream

The dispatcher SHALL follow every non-terminal bound operation by consuming Platform's per-operation SSE event stream, mapping each frame onto the internal projection seam with the frame identifier as the deduplicating event id and the frame timestamp as occurrence time, resuming with the last seen event identifier after a reconnect, and stopping only when durable binding state reports a terminal status or shutdown cancellation is received. Transient owner lookup, session exchange, authentication, stream-open, and nonterminal stream-close outcomes SHALL leave the binding eligible for bounded resumption without process restart. Each stream open or reconnect SHALL obtain a currently valid Platform session. Concurrent scans SHALL still run at most one follower per live operation.

#### Scenario: Frames become throttled projections
- **WHEN** a followed operation's stream delivers accepted then running frames
- **THEN** the bound message is edited according to the existing projection guards and throttle arithmetic

#### Scenario: A redelivered frame changes nothing twice
- **WHEN** a reconnect replays a frame whose identifier was already consumed
- **THEN** the replayed frame is dropped by event deduplication and no extra edit is enqueued

#### Scenario: A restarted dispatcher follows each live operation once
- **WHEN** the dispatcher restarts while three operations sit non-terminal and one terminal in its bindings
- **THEN** it opens streams for exactly the three non-terminal operations and none for the terminal one

#### Scenario: Temporary session exchange failure recovers in-process
- **WHEN** a live operation's first session exchange fails transiently and a later exchange succeeds
- **THEN** a later bounded follow attempt opens the stream without requiring dispatcher restart

#### Scenario: Reconnect refreshes an expired session
- **WHEN** a live stream must reconnect after its Platform session expires
- **THEN** the reconnect uses a fresh valid session and resumes from the last seen event identifier
