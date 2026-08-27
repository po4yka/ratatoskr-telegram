# Telegram integration testing strategy

Required tests:

- Webhook secret, malformed/oversized body, duplicate/out-of-order updates, fast acknowledgment, restart/redelivery.
- Identity binding, access allow/deny, private/group/thread context, account changes.
- Command/message URL/file/forward classification and dialogue expiry/cancel/concurrency.
- Callback token scope/expiry/single-use/replay and opaque deep-link intents.
- Mini App `initData` valid/invalid signature, stale auth time, wrong bot/audience, replay, user mismatch.
- Article flow and GitHub metadata/track/star confirmation/partial-result matrices, including a
  fake Platform/GitHub server that counts action calls and proves preview and selection perform zero.
- Message projection ordering, stale events, send/edit failure, deletion, retry-after, global/per-chat limits.
- Notification preferences/privacy, safe escaping, no-secret/content logging.
- current-schema creation, outbox/inbox replay, mock Bot API, and workspace Telegram -> Platform -> domain flow.

Fixtures use synthetic IDs/updates/files and a mock Bot API; no production bot token.

## Test-first

A change is planned before it is built, and the plan is a task list in which behaviour arrives in
pairs: one task adds a failing test, the next makes it pass. `openspec/config.yaml` carries that
rule, which is what puts it into every planning and implementation request rather than only into this
document.

The loop:

1. Write the test the scenario names. Run it. Confirm it fails, and read the failure — a test that
   fails because it does not compile has proved nothing about the behaviour.
2. Write the smallest change that makes it pass. Run it again.
3. Refactor only once it is green, adding no test and changing no behaviour.

Two checks stand behind this, and neither of them can see the order:

- `openspec validate --archived`, in `.github/workflows/openspec.yml`, fails when a change was
  archived with a task left unticked.
- A step in `.github/workflows/fleet.yml` fails when this repository holds a manifest and a `ci.yml`
  that never runs a test.

`ratatoskr-workspace/docs/QUALITY_GATES.md` records why the order itself is not checkable, rather
than leaving the gap to be discovered.
