## Why

Dispatcher workers are spawned as detached tasks, while HTTP shutdown closes shared resources without cancelling or joining them. A graceful stop can therefore terminate in-flight Bot API or Platform work at an uncertain boundary and leave claims or acknowledgements inconsistent.

## What Changes

- Make the dispatcher runtime own cancellation and join handles for outbound, event-consumer, operation-follower, and notification workers.
- On shutdown, stop new claims, let admitted in-flight work reach its durable settlement boundary, and join workers before closing database and telemetry resources.
- Bound the drain interval and abort only after its deadline, preserving recoverable durable state for unfinished work.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `operational-recovery`: graceful dispatcher shutdown coordinates worker cancellation and bounded drain before shared resources close.
- `outbound-delivery`: outbound workers stop claiming new jobs and settle admitted work during shutdown.

## Impact

- Dispatcher runtime construction, worker loop cancellation, HTTP/process shutdown ordering, and lifecycle integration tests.
- Implementation follows the ambiguous-send fix and precedes the transient-follower fix, which
  then adopts this owned cancellation boundary.
- No public API, schema, or dependency changes are expected.
