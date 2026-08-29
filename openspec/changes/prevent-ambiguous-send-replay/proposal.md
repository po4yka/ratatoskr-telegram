## Why

Telegram `sendMessage` has no idempotency or reconciliation key. If Telegram acknowledges a send but local acknowledgement persistence fails, the current stale-lease recovery can send the message again and create a user-visible duplicate.

## What Changes

- Atomically persist all local state derived from a known Telegram send acknowledgement and retry that local commit while the provider outcome remains known.
- Classify network interruption, process loss, and stale non-idempotent sends with no persisted acknowledgement as delivery outcome unknown and quarantine them from automatic replay.
- Continue bounded automatic retries only when Telegram definitively reports that a send was not applied; retain safe edit retry behavior.
- Expose unknown outcomes for operator inspection and recovery without private message content.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `outbound-delivery`: non-idempotent sends distinguish definite refusal from ambiguous delivery and never automatically replay an ambiguous outcome.
- `operational-recovery`: unknown send outcomes are inspectable and quarantined rather than reclaimed as ordinary stale work.
- `persistence-schema`: outbound state represents an explicit unknown-delivery terminal class and atomically records known acknowledgements.

## Impact

- Dispatcher send processing, outbound persistence state/transitions, operator projections, schema definition, and fake Bot API integration tests.
- **BREAKING** operational behavior: an ambiguous `sendMessage` is quarantined for inspection instead of automatically retried.
- No Telegram API or external dependency changes.
