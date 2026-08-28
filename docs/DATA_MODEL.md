# Telegram integration data model

## Owned schema: `telegram.*`

- `identities`: Telegram user ID, internal user binding, display snapshot, access/status.
- `chats`: chat/thread type, binding, permissions, notification policy.
- `updates`: update ID, safe type/hash, received/processed status, attempts/error.
- `interactions` and `dialog_states`: versioned, expiring interaction state. The current
  `github_repository` kind stores a bounded typed target/account/selection/result payload, stable
  action identity, expected acknowledged message, lifecycle, and terminal time.
- `interaction_tokens`: exact 64-character URL-safe random authority shared by `callback`,
  `deep_link`, and `command` surfaces. Every row binds bot/user/chat and, for callbacks,
  message/dialogue/version;
  it stores expiry plus paired one-time consumption evidence. Operation-status deep links retain
  either a source URL or typed blob facts and minimized forwarded-message provenance server-side.
  A `library_read` command row stores only the canonical user and analysis references needed for
  the action; it carries no title, snippet, document reference, or query and expires after 15 minutes.
- `message_bindings`: operation/entity -> chat/message/thread, last projection/version.
- notification preferences, outbound queue/delivery attempts, outbox/inbox.

## Constraints

Provider IDs are not internal user IDs. Bot token/webhook secret are secret configuration, not rows/events. Raw message/file content is minimized and delegated through authorized blob references. Tokens are high-entropy, single-use, and expiring. Callback authorization checks the owner, bot, private chat, provider-acknowledged message, dialogue step, and version in one transaction. Projection versions increase monotonically. Cross-schema writes/foreign keys are forbidden. A bounded transaction expires active dialogues, deletes only expired/consumed/stale tokens, and removes terminal dialogues only after retention and token removal; domain operations and message bindings are untouched.

Library titles and snippets exist only in the bounded Platform response and the direct outbound job
needed to deliver it; they are never copied into token rows or ordinary telemetry. Expired or
consumed library-read tokens follow the existing bounded interaction cleanup and do not mutate
Knowledge state during deletion.

For capture intents, an attachment payload has a null source URL and bounded `metadata` containing its
`BlobRef` facts (owner, SHA-256 digest, media type, length); a forwarded capture records only
available origin identifiers and timestamp. Local blob-store paths, Bot API download paths and raw
attachment bytes are never intent data.
