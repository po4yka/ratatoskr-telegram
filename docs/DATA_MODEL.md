# Telegram integration data model

## Owned schema: `telegram.*`

- `identities`: Telegram user ID, internal user binding, display snapshot, access/status.
- `chats`: chat/thread type, binding, permissions, notification policy.
- `updates`: update ID, safe type/hash, received/processed status, attempts/error.
- `interactions` and expiring `dialog_states`.
- `callback_tokens`: opaque token hash, user/chat/action scope, payload blob/ref, expiry/consumed.
- `interaction_intents`: opaque deep-link token, owner/kind/payload, expiry/consumed; captures
  retain either a source URL or typed blob facts, plus minimized forwarded-message provenance.
- `message_bindings`: operation/entity -> chat/message/thread, last projection/version.
- notification preferences, outbound queue/delivery attempts, outbox/inbox.

## Constraints

Provider IDs are not internal user IDs. Bot token/webhook secret are secret configuration, not rows/events. Raw message/file content is minimized and delegated through authorized blob references. Tokens are high-entropy, stored hashed where possible, single-use/expiring. Projection versions increase monotonically. Cross-schema writes/foreign keys are forbidden. Retention bounds updates/dialogues/callbacks/intents while preserving necessary audit.

For plan item 6, an attachment intent has a null source URL and bounded `metadata` containing its
`BlobRef` facts (owner, SHA-256 digest, media type, length); a forwarded capture records only
available origin identifiers and timestamp. Local blob-store paths, Bot API download paths and raw
attachment bytes are never intent data.
