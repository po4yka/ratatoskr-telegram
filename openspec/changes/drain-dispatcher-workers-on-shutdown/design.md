## Context

The shared HTTP lifecycle accepts a background factory that returns no owned runtime. Dispatcher startup drops sender and consumer handles and separately spawns follower and notification supervisors. Shutdown drains listeners, then closes PostgreSQL and telemetry while those tasks may still claim or settle work.

## Goals / Non-Goals

**Goals:**

- Give process shutdown one owner for every dispatcher background task.
- Stop new admissions immediately and preserve bounded time for already admitted durable work.
- Close shared resources only after all workers exit or are aborted and awaited.

**Non-Goals:**

- Waiting without a deadline.
- Draining work not yet claimed when shutdown begins.
- Hiding fundamentally ambiguous provider outcomes during forced termination.

## Decisions

### D1: Background startup returns an owned runtime

The HTTP process lifecycle receives a `BackgroundRuntime` containing a synchronously sealed
admission state, a root cancellation channel, and every direct worker handle. Dispatcher
construction registers outbound sender, projection consumer, follower, and notification workers
as children. Dropping individual handles is prohibited by the interface.

A global task registry was rejected because ownership and test isolation would be implicit. Per-worker ad hoc shutdown channels were rejected because ordering and completion remain fragmented.

### D2: Cancellation stops admission, not arbitrary critical sections

Worker loops select cancellation before the next database claim, scan, or transport fetch. Sender
claims and follower spawns also enter an admission reader section and check the synchronous seal;
the shutdown request closes that seal before returning and owns the writer-fence task inside the
same grace budget. Once a send wire request starts, its response classification and known-ack local
transaction receive the remaining grace interval. Follower streams can stop promptly because last
event id and inbox deduplication recover them. The event consumer drains already accepted feed
items after its producers stop.

### D3: Join precedes shared-resource close

When shutdown begins, the process synchronously seals new admission and signals cancellation while
listeners enter their drain window. After listeners stop accepting, it awaits every owned handle
under the configured grace deadline. At deadline it aborts unfinished children and awaits their
termination. Only then does it stop the prober and close the database and telemetry provider.

### D4: One shutdown path is testable without OS timing

Lifecycle tests trigger cancellation through the same programmatic signal used by the process handler and gate a fake in-flight request. Assertions observe no new claim, durable settlement of the admitted request, child completion, and resource-close ordering.

## Risks / Trade-offs

- [A wire call consumes the full grace interval] -> Bound every network call below the process grace budget and classify remaining state according to outbound semantics.
- [Cancellation ordering deadlocks a consumer] -> Stop producers first, close feed channels, then await consumers under the common deadline.
- [Second termination signal requires fast exit] -> Escalate to abort, but still await handles before closing shared resources.

## Migration Plan

No data migration is required. Land this lifecycle foundation before adapting follower and send semantics to its critical-section boundaries. Rollback is code-only but restores detached-task shutdown risk.
