# Telegram integration data model

## Owned schema: `telegram.*`

- `identities`: Telegram user ID, internal user binding, display snapshot, access/status.
- `chats`: chat/thread type, binding, permissions, notification policy.
- `updates`: update ID, safe type/hash, received/processed status, attempts/error.
- `interactions` and expiring `dialog_states`.
- `callback_tokens`: opaque token hash, user/chat/action scope, payload blob/ref, expiry/consumed.
- `interaction_intents`: opaque deep-link token, owner/kind/payload, expiry/consumed.
- `message_bindings`: operation/entity -> chat/message/thread, last projection/version.
- notification preferences, outbound queue/delivery attempts, outbox/inbox.

## Constraints

Provider IDs are not internal user IDs. Bot token/webhook secret are secret configuration, not rows/events. Raw message/file content is minimized and delegated through authorized blob references. Tokens are high-entropy, stored hashed where possible, single-use/expiring. Projection versions increase monotonically. Cross-schema writes/foreign keys are forbidden. Retention bounds updates/dialogues/callbacks/intents while preserving necessary audit.
