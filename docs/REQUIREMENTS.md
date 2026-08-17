# Telegram integration requirements

## Goals

1. Accept Bot API updates through a secure, fast webhook.
2. Let authorized users submit article URLs/files and GitHub repositories through bot commands/messages.
3. Provide confirmations, progress, partial results, and notifications through editable message projections.
4. Authenticate a Telegram Mini App and exchange a short-lived identity assertion/session through Platform.
5. Support opaque callback/deep-link intents and durable dialogue state.

## Non-goals

Owning article/GitHub/Vault/Knowledge data, storing provider credentials, direct cross-schema writes, reading user dialogs via MTProto, or performing long extraction/provider work inside webhook handlers.

## Requirements

- Webhook secret and update deduplication precede processing; acknowledgment is fast.
- Telegram identity is bound explicitly to an internal user and access policy.
- Callbacks/intents are opaque, expiring, scoped, and replay-safe.
- Mini App raw `initData` is verified server-side including time/replay/audience.
- Domain work becomes Platform/typed commands and operation projections.
- GitHub `star` and other writes require explicit confirmation and truthful partial results.
- Outbound edits/sends are ordered, idempotent, rate-limited, and retry-safe.

First slice: authorized user sends article URL -> operation/progress -> one edited final message and Mini App deep link.
