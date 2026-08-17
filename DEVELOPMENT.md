# Developing Ratatoskr Telegram

> Status: Proposed  
> Last reviewed: 2026-08-17

Architecture bootstrap: webhook, dispatcher, Bot API client, schema, dialogue engine, Mini App validator, and deployment are not implemented.

## Intended toolchain

Rust/Tokio, Axum/Tower, Telegram Bot API over Reqwest, SQLx/PostgreSQL, NATS JetStream, typed callback/deep-link intents, Mini App `initData` verification, tracing, WireMock, and testcontainers.

## Workflow

1. Authenticate webhook transport and deduplicate each update before side effects.
2. Acknowledge quickly; process durable interactions asynchronously.
3. Keep Telegram identity/interaction/message projection state separate from article, GitHub, Vault, and Knowledge authority.
4. Use short opaque callback/deep-link tokens and validate Mini App identity server-side.
5. Test replay, ordering, callback expiry, rate limits, partial domain results, edits/deletes, restart, and reauthorization.

The first scaffold PR must document exact local webhook/tunnel, bot sandbox, migration, test, and deployment commands. Default CI uses synthetic updates and never a production bot token.
