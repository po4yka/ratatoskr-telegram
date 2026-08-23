-- The Telegram database, in one file.
--
-- `ratatoskr-telegram` applies this at startup, to a fresh database. There is no migration ledger
-- and no incremental history: no database holds data that has to survive a schema change. A schema
-- change edits this file in place; the next fresh database has it.
--
-- One schema, and it is the only one this service may create or touch. `docs/DATA_MODEL.md` names
-- the tables the bounded context will own — identities, chats, updates, interactions, dialog
-- states, intents, callback tokens, message bindings, notification preferences, delivery state,
-- outbox and inbox — and each arrives with the plan item that owns its first writer:
--
--   * `telegram` — Telegram interaction state: update deduplication, identity/chat binding,
--     dialogue state, opaque intents and callback tokens, message projections, outbound queue,
--     notification preferences, and the outbox/inbox pair.
--
-- Conventions, applied uniformly and stated once here (the fleet's, from ratatoskr-platform):
--
--   * Identifiers are UUIDv7 minted by the application, never by the database. There is deliberately
--     no DEFAULT on any id column, so a missing id is a compile-or-insert error rather than a
--     silently wrong version.
--
--   * Closed vocabularies are `text` with a CHECK, not a PostgreSQL enum.
--
--   * Every timestamp is `timestamptz`. `timestamp` would silently record the server's local time.
--
--   * No credential is stored in a readable form anywhere in this file, ever.
--
--   * No foreign key crosses a schema boundary. References to Platform-owned identifiers are
--     unenforced columns; the application enforces them.

-- =================================================================================================
-- telegram
-- =================================================================================================

create schema telegram;

comment on schema telegram is
    'Telegram-owned interaction state for Ratatoskr: update deduplication, identity and chat '
    'bindings, dialogue state, opaque intents and callback tokens, message projections, the '
    'outbound queue with its rate-limit state, notification preferences, and the outbox/inbox '
    'pair. Tables arrive with the features that own their first writer; nothing here is shared '
    'with or referenced by another service''s schema.';

-- `updates` — one row per ADMITTED Bot API update, written by the webhook before any processing
-- handoff. The composite key IS the deduplication decision: insert-or-ignore over
-- (bot_id, update_id) makes redelivered and out-of-order duplicates no-ops while genuinely unseen
-- ids — including ones below the highest seen id — still insert. `kind` holds a CLOSED
-- classification label (`message`, `callback_query`, ..., `unsupported`). The authenticated,
-- parsed payload remains processable across a restart until terminal settlement removes it.
-- `state` walks accepted -> processing -> exactly one of processed / unsupported / failed.
create table telegram.updates (
    bot_id      bigint      not null,
    update_id   bigint      not null,
    kind        text        not null,
    payload     jsonb,
    state       text        not null default 'accepted'
                check (state in ('accepted', 'processing', 'processed', 'unsupported', 'failed')),
    received_at timestamptz not null default now(),
    settled_at  timestamptz,
    primary key (bot_id, update_id),
    constraint update_payload_exists_only_while_processable
        check ((state in ('accepted', 'processing')) = (payload is not null))
);

comment on table telegram.updates is
    'Admitted Bot API updates and their processing state. One row per (bot, update id); the '
    'primary key is the deduplication identity. Payload is retained only while processable.';
