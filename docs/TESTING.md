# Telegram integration testing strategy

Required tests:

- Webhook secret, malformed/oversized body, duplicate/out-of-order updates, fast acknowledgment, restart/redelivery.
- Identity binding, access allow/deny, private/group/thread context, account changes.
- Command/message URL/file/forward classification and dialogue expiry/cancel/concurrency.
- Callback token scope/expiry/single-use/replay and opaque deep-link intents.
- Mini App `initData` valid/invalid signature, stale auth time, wrong bot/audience, replay, user mismatch.
- Article flow and GitHub metadata/track/star/list/policy confirmation/partial-result matrices.
- Message projection ordering, stale events, send/edit failure, deletion, retry-after, global/per-chat limits.
- Notification preferences/privacy, safe escaping, no-secret/content logging.
- SQL migrations, outbox/inbox replay, WireMock Bot API, and workspace Telegram -> Platform -> domain flow.

Fixtures use synthetic IDs/updates/files and a mock Bot API; no production bot token.
