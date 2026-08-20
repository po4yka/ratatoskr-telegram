# Developing Ratatoskr Telegram

> Status: Proposed  
> Last reviewed: 2026-08-17

Architecture bootstrap: webhook, dispatcher, Bot API client, schema, dialogue engine, Mini App validator, and deployment are not implemented.

## Intended toolchain

Rust/Tokio, Axum/Tower, Telegram Bot API over Reqwest, SQLx/PostgreSQL, NATS JetStream, typed callback/deep-link intents, Mini App `initData` verification, tracing, WireMock, and testcontainers.

## Code size limits

There is no code here yet, so no limit is enforced yet. The commit that brings the first manifest brings the configuration that carries the limits with it: `clippy.toml` beside a `Cargo.toml`, `eslint.config.js` beside a `package.json`. `fleet.yml` fails the gate when a manifest arrives without one, so the rule has a check behind it and not only this paragraph.

`ratatoskr-workspace/docs/QUALITY_GATES.md` holds the numbers the repositories with code use today, the command that measured each one, and the limits that were rejected with the reason. Read it before you choose numbers, then measure this tree. Each limit is set at the worst case the tree already has, so that the check fails on a regression and not on work that has not been done yet.

## Workflow

1. Authenticate webhook transport and deduplicate each update before side effects.
2. Acknowledge quickly; process durable interactions asynchronously.
3. Keep Telegram identity/interaction/message projection state separate from article, GitHub, Vault, and Knowledge authority.
4. Use short opaque callback/deep-link tokens and validate Mini App identity server-side.
5. Test replay, ordering, callback expiry, rate limits, partial domain results, edits/deletes, restart, and reauthorization.

The first scaffold PR must document exact local webhook/tunnel, bot sandbox, migration, test, and deployment commands. Default CI uses synthetic updates and never a production bot token.
