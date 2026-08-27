# Telegram integration implementation plan

1. Scaffold Rust service, typed config, telemetry, health, errors, and `telegram` schema.
2. Implement Bot API client, secure webhook, body/schema limits, update dedupe, fast acknowledgment.
3. Add identity/chat binding and access-control bootstrap.
4. Implement dispatcher, message bindings, ordered/rate-limited send/edit, and operation projection.
5. Add URL/article capture command/message flow.
6. Add file/forward handling with safe blob handoff.
7. Add GitHub repository preview and `metadata`/`track`/`star` confirmations/partial results. (done)
8. Implement callback tokens, dialogue state, opaque deep-link intents. (done)
9. Implement Mini App `initData` validation and short-lived Platform assertion exchange.
10. Add notifications/preferences, deployment/runbooks, failure recovery, and workspace integration.

Definition of Done: forged/duplicate updates have no effects; webhook stays fast; identities and Mini App auth are secure; callbacks replay-safe; projections ordered/rate-limited; article/GitHub flows, current-schema tests, and workspace integration pass. Deferred: MTProto/userbot, payments, and broad group administration.
