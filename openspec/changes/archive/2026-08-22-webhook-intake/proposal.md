# Secure webhook intake: Bot API client, admission limits, update deduplication, fast acknowledgment

## Why

Plan item 2 of `docs/IMPLEMENTATION_PLAN.md`. The scaffold answers only its operator plane; no code
path contacts Telegram, and nothing persists an interaction. Until a secret-verified webhook exists
that deduplicates updates before any side effect and acknowledges Telegram faster than it does any
work, every later item (identity binding, commands, projections) would be bolted onto an intake
that has never been proven safe against forged, oversized, malformed or redelivered deliveries.

The legacy monolith used a long-polling Telethon user session. This repository deliberately uses
Bot API webhook mode instead: Telegram pushes updates to us over HTTPS we admit under strict
control, rather than us pulling from a user account.

## What Changes

- Add the `ratatoskr-telegram-bot-api` crate: a typed Bot API client over the pinned `teloxide`
  dependency, exposing `get_me`, `set_webhook`, `send_message`, `edit_message_text`,
  `answer_callback_query` and `send_chat_action`, with a configurable base URL so tests and local
  runs point at a harness server instead of Telegram. It also owns update payload parsing
  (`teloxide::types::Update`) with recorded synthetic fixtures.
- Add the webhook public listener to the webhook role: one route that verifies
  `X-Telegram-Bot-Api-Secret-Token` in constant time BEFORE reading or parsing anything, restricts
  method and content type, enforces a configured body-size cap on declared and streamed bodies,
  parses the update against the Bot API schema, and returns 200 without waiting for any downstream
  work.
- Persist update deduplication: a new `telegram.updates` table keyed `(bot_id, update_id)`;
  insert-or-ignore decides accepted versus duplicate, so redelivered and out-of-order older
  duplicates are dropped idempotently while genuinely unseen older ids are still processed.
- Hand accepted updates to an in-process bounded queue consumed by a worker task after the response
  is sent; the worker classifies the update kind and settles the row's processing state. Queue
  saturation and storage failure answer 503 so Telegram retries.
- Typed admission outcomes: unauthorized 401, non-POST 405, wrong content type 415, oversized 413
  with an explicit limit response, malformed/schema-invalid JSON logged and ACKED 200 to prevent
  retry storms.
- Configuration: new `bot_api` (base URL, timeout, bot token as a secret) and `webhook` (public
  bind, secret token as a secret, max body bytes) tables, validated by rules V9–V13 appended to the
  existing value-free report; the webhook role now REQUIRES its database, refusing to start when the
  database cannot be reached, because intake writes through the pool.
- Telemetry: request-outcome, received-update-kind and admission-duration instruments with bounded
  label vocabularies.
- The lifecycle harness (`telegram_http::run`) gains an optional public router factory so both
  binaries keep one shutdown sequence; the dispatcher passes none.

## Capabilities

### New Capabilities

- `update-intake`: how a delivered update is admitted — secret verification, limits, schema
  checking, deduplication, fast acknowledgment, and the typed outcomes for each rejection class.
- `bot-api-client`: the typed client boundary this service calls Telegram through, and how it is
  tested without contacting Telegram.

### Modified Capabilities

- `persistence-schema`: adds the `telegram.updates` table, the first writer owned by intake.
- `service-configuration`: adds the `bot_api` and `webhook` configuration tables, their validation
  rules, and the webhook role's requirement of a reachable database at startup.
- `telemetry`: adds the intake instruments to the bounded metric registry.

## Impact

- New crate `crates/bot-api`; `services/webhook` gains a library target holding the intake
  pipeline; `crates/core`, `crates/persistence`, `crates/telemetry`, `crates/http` extended in
  place; both binaries' `main` updated to the new `run` contract.
- `schema.sql` edited in place (development status: no migrations); fresh databases get
  `telegram.updates`.
- Dependencies moved onto the audit surface: `teloxide` (+ its graph), `reqwest`, `subtle`,
  `http-body-util` promoted from dev-only.
- Behaviour change: the webhook binary no longer starts without a configured, reachable database;
  boot expectations updated accordingly (the dispatcher keeps the old tolerance until its own item).
- Out of scope, unchanged: command parsing, dispatching/projection, identity binding and access
  control (items 3–4). The worker settles classification state only; it performs no domain action.
