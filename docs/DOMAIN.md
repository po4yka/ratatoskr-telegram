# Telegram integration domain model

## Terms

- **Telegram identity:** provider user ID bound to internal user and access state.
- **Chat binding:** allowed private/group chat and notification policy.
- **Update record:** deduplicated Bot API update and processing status.
- **Interaction:** user intent derived from message/command/callback/Mini App.
- **Dialogue state:** expiring multi-step input/confirmation context.
- **Callback token:** opaque short payload resolving to authorized server-side action.
- **Deep-link intent:** opaque expiring Mini App start token and payload.
- **Message binding:** operation/domain projection mapped to chat/message/thread.
- **Notification preference:** event classes, destinations, and quiet behavior.

## Invariants

1. Telegram owns interaction/projection state only.
2. Every update ID is processed idempotently.
3. Long work is asynchronous after fast acknowledgment.
4. Callback/deep-link payloads contain no secrets/raw domain state.
5. Mini App identity is server-verified, not trusted from `initDataUnsafe`.
6. Provider writes require explicit confirmation.
7. Outbound state never regresses a terminal operation because of stale events.
