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

-- `identities` — one row per Telegram user this deployment has admitted or evaluated about
-- access. Rows are created by the startup bootstrap from configuration (the owner) and later by
-- explicit enrollment flows; the authorization gate only reads them and never enrolls on
-- first contact. `access_state`
-- carries the deployment's decision (`enabled`, `disabled`); it defaults to `enabled` because a
-- row only exists once something admitted its subject or the owner bootstrap created it.
-- `internal_user_id` is an unenforced reference into Platform's identity domain — no foreign key
-- crosses a schema boundary — and stays NULL until identity binding lands. The profile snapshot
-- columns are display evidence from the update that created the row; they are never treated as
-- authenticated actor identity.
create table telegram.identities (
    telegram_user_id bigint      not null primary key,
    internal_user_id uuid,
    username         text,
    first_name       text,
    last_name        text,
    access_state     text        not null default 'enabled'
                     check (access_state in ('enabled', 'disabled')),
    created_at       timestamptz not null default now(),
    updated_at       timestamptz not null default now()
);

comment on table telegram.identities is
    'Telegram users known to this deployment and their access state. internal_user_id is an '
    'unenforced reference into Platform''s identity domain until binding lands.';

-- `chats` — one row per chat this deployment has evaluated. Only private conversations are
-- representable in this version: group and channel support waits for an explicit design, so the
-- vocabulary is closed at `private` and the gate denies every other chat type without creating a
-- row for it. Like identities, rows appear lazily when the gate first admits a private
-- conversation.
create table telegram.chats (
    chat_id      bigint      not null primary key,
    type         text        not null check (type in ('private')),
    access_state text        not null default 'enabled'
                 check (access_state in ('enabled', 'disabled')),
    created_at   timestamptz not null default now(),
    updated_at   timestamptz not null default now()
);

comment on table telegram.chats is
    'Chats this deployment has evaluated. Closed at private for this version; other chat types '
    'are denied at the gate and gain no row.';

-- `updates` — one row per ADMITTED Bot API update, written by the webhook before any processing
-- handoff. The composite key IS the deduplication decision: insert-or-ignore over
-- (bot_id, update_id) makes redelivered and out-of-order duplicates no-ops while genuinely unseen
-- ids — including ones below the highest seen id — still insert. `kind` holds a CLOSED
-- classification label (`message`, `callback_query`, ..., `unsupported`). The authenticated,
-- parsed payload remains processable across a restart until terminal settlement removes it.
-- `state` walks accepted -> processing -> exactly one of processed / unsupported / failed /
-- denied. `denied` records a sender or chat the access policy refused before any processing ran:
-- it is settled like any terminal state — settle time stamped, processable payload removed — and
-- the silent, no-outbound-call part of the refusal lives in the webhook worker, not here.
create table telegram.updates (
    bot_id      bigint      not null,
    update_id   bigint      not null,
    kind        text        not null,
    payload     jsonb,
    state       text        not null default 'accepted'
                check (state in ('accepted', 'processing', 'processed', 'unsupported',
                                 'failed', 'denied')),
    received_at timestamptz not null default now(),
    settled_at  timestamptz,
    primary key (bot_id, update_id),
    constraint update_payload_exists_only_while_processable
        check ((state in ('accepted', 'processing')) = (payload is not null))
);

comment on table telegram.updates is
    'Admitted Bot API updates and their processing state. One row per (bot, update id); the '
    'primary key is the deduplication identity. Payload is retained only while processable.';

-- `message_bindings` — one live binding of a Platform operation to one Telegram chat message.
-- The dispatcher edits that message in place as operation events arrive; the unique constraint on
-- (operation_id, chat_id) makes "one live binding" a database fact rather than a convention.
-- `operation_id` is an unenforced reference into Platform's operation domain — no foreign key
-- crosses a schema boundary — and the application owns the invariant it names. `message_id` is
-- NULL until a send is acknowledged by the Bot API, and NULL again after an unbind (a permanent
-- edit failure clears it so the next revision sends a fresh message and rebinds); provider
-- message ids are recorded only after acknowledgment, never from an attempt still in flight.
-- Revisions are monotonic per binding: `last_rendered_revision` only ever moves forward, and
-- `last_rendered_at` records when the newest accepted revision was rendered. `last_event_at` is
-- the accept-side watermark — the occurred_at of the newest ACCEPTED event — so staleness is
-- judged against what was accepted, not only what was delivered.
create table telegram.message_bindings (
    id                     uuid        not null primary key,
    bot_id                 bigint      not null,
    operation_id           uuid        not null,
    chat_id                bigint      not null,
    message_id             bigint,
    last_rendered_revision bigint      not null default 0,
    last_rendered_at       timestamptz,
    last_event_at          timestamptz,
    terminal               boolean     not null default false,
    created_at             timestamptz not null default now(),
    updated_at             timestamptz not null default now(),
    unique (operation_id, chat_id)
);

comment on table telegram.message_bindings is
    'One live Telegram message per Platform operation and chat: the anchor the dispatcher edits '
    'as progress arrives. operation_id is an unenforced reference into Platform''s operation '
    'domain; message_id is NULL until a send is acknowledged and NULL again after an unbind.';

comment on column telegram.message_bindings.terminal is
    'Set once when a terminal projection is accepted; every later event for the binding is '
    'dropped, so a terminal render can never be overwritten.';

comment on column telegram.message_bindings.last_event_at is
    'occurred_at of the newest accepted event; the staleness watermark for the guard sequence. '
    'Advances only when an event is accepted, never on duplicates or stale drops.';

-- `outbound_jobs` — the durable queue of Bot API writes. Every sendMessage and editMessageText
-- the service will ever make is a row here before any network call, so a crash between
-- acceptance and delivery loses nothing: a restart claims from this table again. `id` is a
-- UUIDv7 minted at enqueue; UUIDv7 sorts by creation time, so id order within a chat
-- approximates enqueue order and the per-chat FIFO claim needs no separate sequence column.
-- `payload` holds the whole rendered message — text, parse mode, and inline keyboard when the
-- render carries one — so markup survives queueing, restarts, and retries bit-identically, and
-- is content-bearing: pruning aged rows later is a stated retention duty, not something this
-- schema performs today. `content_hash` is the sha256 hex of the canonical payload
-- serialization, computed by the caller, so an identical re-render is detectable without diffing.
-- `state` uses ARCHITECTURE.md §18.1's exact tokens — these are vocabulary, not synonyms
-- to be improved locally. `lease_expires_at` is NULL unless the row is claimed for sending; a
-- stale lease is what makes a crashed sender's job claimable again. `last_error_class` records a
-- closed safe class label at dead-lettering, never provider error text.
create table telegram.outbound_jobs (
    id               uuid        not null primary key,
    bot_id           bigint      not null,
    chat_id          bigint      not null,
    kind             text        not null check (kind in ('send_message', 'edit_message_text')),
    payload          jsonb       not null,
    content_hash     text        not null,
    operation_id     uuid,
    revision         bigint,
    correlation_id   text,
    delivery_class   text        not null default 'direct'
                     check (delivery_class in ('direct', 'projection', 'notification')),
    notification_id  uuid,
    notification_created_at timestamptz,
    state            text        not null default 'ready'
                     check (state in ('planned', 'ready', 'sending', 'sent', 'retry_wait',
                                      'superseded', 'failed_permanent', 'cancelled')),
    attempts         integer     not null default 0,
    next_attempt_at  timestamptz not null default now(),
    lease_expires_at timestamptz,
    last_error_class text,
    created_at       timestamptz not null default now(),
    updated_at       timestamptz not null default now(),
    check ((delivery_class = 'notification') = (notification_id is not null)),
    check ((delivery_class = 'notification') = (notification_created_at is not null))
);

-- The due scan behind every claim: ready and waiting jobs ordered by their earliest attempt.
create index outbound_jobs_due_idx on telegram.outbound_jobs (next_attempt_at)
    where state in ('ready', 'retry_wait');
-- The per-chat FIFO head read: lowest id per chat among eligible rows.
create index outbound_jobs_chat_idx on telegram.outbound_jobs (chat_id, id);
-- The lease scan: rows in flight whose sender may have died.
create index outbound_jobs_sending_idx on telegram.outbound_jobs (state)
    where state = 'sending';
create unique index outbound_jobs_notification_idx
    on telegram.outbound_jobs (notification_id, chat_id)
    where notification_id is not null;

comment on table telegram.outbound_jobs is
    'The durable Bot API write queue: one row per planned or attempted sendMessage / '
    'editMessageText, claimed strictly FIFO per chat with at most one job in flight per chat.';

comment on column telegram.outbound_jobs.id is
    'UUIDv7 minted at enqueue; v7 sorts by time, so id order within a chat approximates enqueue '
    'order and the FIFO claim reads no separate sequence column.';

comment on column telegram.outbound_jobs.payload is
    'The whole rendered message - text, parse mode, inline keyboard when one rides along - as '
    'jsonb, restored bit-identically across queueing, restarts, and retries. Content-bearing: '
    'pruning aged rows is a stated retention duty this schema does not yet perform.';

comment on column telegram.outbound_jobs.state is
    'ARCHITECTURE.md §18.1 job-state tokens: planned -> ready -> sending -> sent, plus '
    'retry_wait, superseded, failed_permanent, cancelled.';

comment on column telegram.outbound_jobs.last_error_class is
    'A closed safe failure-class label recorded at dead-lettering; never provider error text.';

-- `inbox` — event-id deduplication for at-least-once event consumption. One row per consumed
-- envelope event id; like updates, the primary key IS the decision, insert-or-ignore, so a
-- redelivered event changes nothing twice without any check-then-insert race.
create table telegram.inbox (
    event_id uuid        not null primary key,
    seen_at  timestamptz not null default now()
);

comment on table telegram.inbox is
    'Envelope event ids already consumed from at-least-once transports; the primary key is the '
    'deduplication decision.';

-- `private_chat_bindings` is the explicit authority connecting an admitted Telegram actor to a
-- private delivery venue. Telegram happens to make many private chat ids numerically equal to a
-- user id; this relation deliberately does not rely on that provider coincidence.
create table telegram.private_chat_bindings (
    telegram_user_id bigint      not null references telegram.identities(telegram_user_id),
    chat_id          bigint      not null references telegram.chats(chat_id),
    bound_at         timestamptz not null,
    primary key (telegram_user_id, chat_id),
    unique (chat_id)
);

comment on table telegram.private_chat_bindings is
    'Explicit admitted actor-to-private-chat delivery authority; numeric id equality is never '
    'authorization.';

-- One global Telegram notification policy per explicit private-chat binding. `inherit` accepts a
-- producer quiet-hours hint, `custom` uses the two local minute offsets, and `disabled` has no
-- quiet window. Equal endpoints are invalid rather than ambiguously meaning never or always.
create table telegram.notification_preferences (
    telegram_user_id     bigint      not null,
    chat_id              bigint      not null,
    enabled              boolean     not null default true,
    quiet_policy         text        not null default 'inherit'
                         check (quiet_policy in ('disabled', 'inherit', 'custom')),
    quiet_start_minute   smallint    check (quiet_start_minute between 0 and 1439),
    quiet_end_minute     smallint    check (quiet_end_minute between 0 and 1439),
    high_priority_bypass boolean     not null default false,
    version              bigint      not null default 0 check (version >= 0),
    created_at           timestamptz not null default now(),
    updated_at           timestamptz not null default now(),
    primary key (telegram_user_id, chat_id),
    foreign key (telegram_user_id, chat_id)
        references telegram.private_chat_bindings(telegram_user_id, chat_id),
    check (
        (quiet_policy = 'custom'
         and quiet_start_minute is not null
         and quiet_end_minute is not null
         and quiet_start_minute <> quiet_end_minute)
        or
        (quiet_policy in ('disabled', 'inherit')
         and quiet_start_minute is null
         and quiet_end_minute is null)
    )
);

create table telegram.notification_class_preferences (
    telegram_user_id bigint      not null,
    chat_id          bigint      not null,
    class            text        not null
                     check (class ~ '^[a-z][a-z0-9_-]{0,31}$'),
    enabled          boolean     not null,
    updated_at       timestamptz not null default now(),
    primary key (telegram_user_id, chat_id, class),
    foreign key (telegram_user_id, chat_id)
        references telegram.notification_preferences(telegram_user_id, chat_id)
        on delete cascade
);

-- A decision is the notification dedupe authority. The carrying transport event id is optional
-- evidence only; replaying the same notification under a different envelope cannot send twice.
create table telegram.notification_decisions (
    id                 uuid        not null primary key,
    notification_id    uuid        not null,
    transport_event_id uuid,
    chat_id            bigint      not null references telegram.chats(chat_id),
    class              text        not null
                       check (class ~ '^[a-z][a-z0-9_-]{0,31}$'),
    outcome            text        not null
                       check (outcome in ('suppressed', 'deferred', 'enqueued', 'delivered',
                                          'retry_wait', 'failed_permanent')),
    outbound_job_id    uuid        references telegram.outbound_jobs(id),
    release_at         timestamptz,
    decided_at         timestamptz not null,
    updated_at         timestamptz not null default now(),
    unique (notification_id, chat_id),
    unique (outbound_job_id),
    check (outcome <> 'deferred' or release_at is not null)
);

create table telegram.notification_transport_failures (
    id              uuid        not null primary key,
    stream_sequence bigint      check (stream_sequence > 0),
    event_id        uuid,
    failure_class   text        not null
                    check (failure_class in ('wrong_event_type', 'invalid_envelope',
                                             'invalid_notification', 'database_unavailable')),
    occurred_at     timestamptz not null
);

comment on table telegram.notification_transport_failures is
    'Content-free poison/transient transport evidence: sequence, event id, safe class and time.';

-- `dialog_states` is the durable, versioned state for finite Telegram interactions. The only
-- dialogue implemented today is the GitHub repository confirmation flow. Its payload is decoded
-- through a closed Rust type; SQL additionally refuses non-object values. Provider credentials,
-- private message bodies, and domain-owned content never belong here.
create table telegram.dialog_states (
    id                     uuid        not null primary key,
    kind                   text        not null check (kind in ('github_repository')),
    bot_id                 bigint      not null,
    telegram_user_id       bigint      not null,
    chat_id                bigint      not null,
    expected_message_id    bigint,
    step                   text        not null check (step in ('preview', 'confirming',
                                                                'submitting', 'completed')),
    version                bigint      not null default 0 check (version >= 0),
    lifecycle              text        not null default 'active'
                                       check (lifecycle in ('active', 'completed', 'cancelled',
                                                            'expired')),
    payload                jsonb       not null check (jsonb_typeof(payload) = 'object'),
    action_idempotency_key text        not null unique,
    created_at             timestamptz not null,
    updated_at             timestamptz not null,
    expires_at             timestamptz not null,
    terminal_at            timestamptz,
    check (expires_at > created_at),
    check ((lifecycle = 'active') = (terminal_at is null)),
    check ((lifecycle = 'completed') = (step = 'completed'))
);

create index dialog_states_expiry_idx
    on telegram.dialog_states (expires_at, id)
    where lifecycle = 'active';

comment on table telegram.dialog_states is
    'Scoped, versioned Telegram dialogue state. Payloads contain bounded references only and are '
    'decoded through the closed type for the named dialogue kind.';

-- `interaction_tokens` is the shared client-presented authority for Telegram callback data and
-- `/start` deep links. The application mints the exact 64-byte random token; no business state is
-- encoded into it. Consumption evidence is paired and scope is checked before either this row or
-- its dialogue can change.
create table telegram.interaction_tokens (
    token                     text        not null primary key
                                          check (octet_length(token) = 64)
                                          check (token ~ '^[A-Za-z0-9_-]{64}$'),
    surface                   text        not null check (surface in ('callback', 'deep_link')),
    action                    text        not null
                                          check (action in ('select_metadata', 'select_track',
                                                             'select_star', 'confirm', 'cancel',
                                                             'operation_status')),
    bot_id                    bigint      not null,
    telegram_user_id          bigint      not null,
    chat_id                   bigint      not null,
    expected_message_id       bigint,
    dialogue_id               uuid references telegram.dialog_states(id) on delete cascade,
    expected_dialogue_version bigint check (expected_dialogue_version >= 0),
    operation_id              uuid,
    payload                   jsonb check (payload is null or jsonb_typeof(payload) = 'object'),
    created_at                timestamptz not null,
    expires_at                timestamptz not null,
    consumed_at               timestamptz,
    consumed_by_user          bigint,
    check (expires_at > created_at),
    check ((consumed_at is null) = (consumed_by_user is null)),
    check (
        (surface = 'callback'
         and action in ('select_metadata', 'select_track', 'select_star', 'confirm', 'cancel')
         and dialogue_id is not null
         and expected_dialogue_version is not null
         and operation_id is null
         and payload is null)
        or
        (surface = 'deep_link'
         and action = 'operation_status'
         and expected_message_id is null
         and dialogue_id is null
         and expected_dialogue_version is null
         and operation_id is not null
         and payload is not null
         and (
             jsonb_exists(payload, 'source_url')
             or coalesce(jsonb_exists(payload->'metadata', 'blob'), false)
         ))
    )
);

create index interaction_tokens_dialogue_idx
    on telegram.interaction_tokens (dialogue_id);

create index interaction_tokens_operation_idx
    on telegram.interaction_tokens (operation_id)
    where surface = 'deep_link';

create index interaction_tokens_cleanup_idx
    on telegram.interaction_tokens (expires_at, token);

comment on table telegram.interaction_tokens is
    'Opaque, expiring, fully scoped callback and deep-link authorities with one-time consumption.';
