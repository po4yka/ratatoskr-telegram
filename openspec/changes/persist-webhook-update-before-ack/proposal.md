## Why

The webhook currently acknowledges an authenticated Telegram update after storing only its identity and classification while the processable payload lives in an in-memory channel. A crash after the insert makes Telegram redelivery look like a duplicate and permanently removes the only payload that the worker could process.

## What Changes

- Persist the authenticated parsed update before returning HTTP success.
- Make the worker claim and load pending updates from PostgreSQL so accepted work survives process restart and an in-memory notification is only a wake-up hint.
- Clear the restricted payload after terminal settlement while retaining the minimized deduplication and audit row.
- Add a regression test that admits an update, drops the original process, and proves a restarted worker settles it exactly once.
- Edit the current schema definition in place; add no migration or production dependency.

## Capabilities

### New Capabilities

- `webhook-update-recovery`: Accepted webhook updates are durably recoverable until terminal processing and minimized afterwards.

### Modified Capabilities

None.

## Impact

Affected surfaces are the private `telegram.updates` schema, persistence crate, and webhook admission/worker code. Telegram's HTTP contract, Bot API authentication, Platform contracts, dispatcher, and deployment remain unchanged.

Rollback is a revert of this repository commit and recreation of the development database from the prior `schema.sql`; the frozen host is not changed.
