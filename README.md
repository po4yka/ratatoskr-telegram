# Ratatoskr Telegram

`ratatoskr-telegram` is the Telegram interaction bounded context for Ratatoskr. It provides a Bot API interface and will provide Telegram Mini App authentication for submitting articles, adding or tracking GitHub repositories, following long-running operations, and receiving notifications from a local Ratatoskr deployment.

> **Status:** plan items 1–8 and 10 implemented; Mini App authentication in item 9 remains gated.
> The service admits and deduplicates secure Bot API
> updates, enforces the private owner access gate, submits URL captures to Platform, and projects
> their progress through a durable outbound queue. Item 6 also captures forwarded external links
> with minimized provenance and accepts bounded PDFs/photos: they stream through the Bot API into
> a Telegram-owned content-addressed blob store, are SHA-256 hashed, and reach Platform as opaque
> `BlobRef` references. Unsupported video, voice, audio, and document types receive one truthful
> response; extraction and transcription remain outside this repository. GitHub confirmations now
> use durable versioned dialogues and exact 64-character, scope-bound, single-use callback tokens;
> `/start` carries only the same kind of opaque server-side intent. It also consumes the fixed
> Platform notification subject, applies private-chat preferences and quiet hours, and dispatches
> admitted messages through the same durable queue. See `DEVELOPMENT.md` for configuration,
> `deploy/README.md` for the single-host profile, and `docs/runbooks/` for recovery.

> [!IMPORTANT]
> **Ratatoskr is in development.** No database holds data that has to survive a schema change.
> While this status holds, these two rules replace what the documents below plan:
>
> - the API and the database keep their first version. There is no `v2` and no later major
>   version.
> - the database has no migrations. One schema definition exists, and a schema change edits it in
>   place.
>
> Only the repository owner changes this status.

## Role in Ratatoskr

Telegram is a first-class integration rather than a generic adapter hidden inside Platform. It owns Telegram-specific credentials, identities, chats, update deduplication, dialogue state, callback confirmations, deep-link intents, and message projections.

It does **not** own:

- article content or extraction results;
- GitHub repositories, stars, lists, or provider tokens;
- Git mirrors or snapshots;
- summaries, embeddings, or search indexes;
- general Ratatoskr collections;
- user account credentials for unrelated providers.

For domain work, Telegram creates authenticated commands through Platform and renders the resulting operation state back into Telegram messages.

## Workspace layout

The workspace exists. Library crates carry the shared concerns; the two planned deployables are
thin binaries over one harness:

```text
ratatoskr-telegram/
├── crates/
│   ├── core/            # runtime role, typed configuration, error taxonomy
│   ├── telemetry/       # tracing subscriber, OTLP export, metrics, build identity
│   ├── http/            # run(role, routes) lifecycle, operator plane, drain-then-close shutdown
│   ├── persistence/     # PostgreSQL pool, embedded schema, durable update admission and claims
│   └── bot-api/         # the typed Bot API client boundary over teloxide (item 2)
├── services/
│   ├── webhook/         # ratatoskr-telegram-webhook: durable intake + recovery worker
│   └── dispatcher/      # ratatoskr-telegram-dispatcher (outbound delivery, projections; item 4)
└── schema.sql           # the first-version `telegram` schema, applied at startup
```

Interaction-domain crates (`interactions`, `mini-app-auth`, `message-projections`) are added by the
plan items that own them, not pre-created empty.

### `ratatoskr-telegram-webhook`

The webhook process:

- accepts Telegram Bot API updates;
- verifies the configured webhook secret;
- persists each parsed payload and deduplication identity before a successful response;
- uses PostgreSQL, not the in-process wake-up queue, as the authority for pending work;
- claims admitted work after restart and settles it to a terminal state;
- removes the processable payload at terminal settlement while retaining deduplication evidence;
- returns a successful HTTP response without waiting for downstream work.

Identity and access resolution are live since item 3: the worker resolves sender and chat records
and settles refusals as `denied` before any domain action. The worker now routes capture,
repository-callback, and opaque `/start` interactions through durable state before settling them.

#### Attachment blob root

`RATATOSKR__INGESTION__BLOB_ROOT` is the absolute, durable Telegram-owned directory used to stage
downloaded attachment bytes before their BlobRef handoff. It defaults to
`/var/lib/ratatoskr-telegram/blobs`; production mounts persistent storage there or sets another
absolute service-owned path. The local path is not a Platform contract and is never included in an
intent, log, or Telegram message.

### `ratatoskr-telegram-dispatcher`

The dispatcher, live since item 4:

- consumes operation and domain events;
- maintains per-chat ordering and rate limits;
- sends initial, progress, final, and warning messages;
- edits existing status messages where appropriate;
- resolves callback-token results;
- retries transient Bot API failures;
- records Telegram message bindings and notification state.

Notification facts arrive only through the documented
`evt.platform.notification.raised.v1` subject and the pre-provisioned
`ratatoskr_telegram_notifications` durable. The dispatcher refuses a missing or incompatible
consumer through readiness; it never creates transport authority itself. `/settings` controls the
global switch, six known class overrides, UTC quiet-hours policy, and explicit high-priority
bypass for the current verified private chat. Unknown future classes inherit the global policy.

For a small self-hosted deployment these may be runtime roles of one binary, but their workload and failure responsibilities remain distinct.

## Bot API, not a userbot

The primary implementation uses the official HTTP Bot API. A Telegram user session through MTProto, Telethon, or TDLib is not required for:

- direct bot conversations;
- commands;
- forwarded messages received by the bot;
- uploaded files;
- inline keyboards;
- Mini App launch and authentication;
- operation notifications.

A future requirement to read the user's private Telegram dialogs or channel history on their behalf would require a separate, explicitly consented MTProto connector with its own credentials and threat model. It must not be silently added to this service.

## Article workflows

The bot accepts several explicit article inputs:

- a plain URL;
- `/article <url>`;
- a forwarded channel message containing a URL;
- several URLs in one interaction;
- a PDF document or photo;
- forwarded or pasted text saved as a note/source.

Typical flow:

```text
Telegram update
  -> ratatoskr-telegram-webhook
  -> authenticated Platform operation
  -> content.extraction.requested.v1
  -> ratatoskr-extractor
  -> knowledge.analysis.requested.v1, when enabled
  -> operation events
  -> ratatoskr-telegram-dispatcher
  -> edited final Telegram message
```

The webhook does not wait for the downstream pipeline. A user may see a message projection such as:

```text
Accepted
Fetching source
Extracting main content
Analysing
Completed
```

The final response may contain:

- title and source;
- concise summary or extraction result;
- completion or partial-warning state;
- `Open`, `Retry`, `Add to collection`, and related actions;
- a deep link into the Mini App or full web client.

## GitHub repository workflows

A bare GitHub repository URL opens the safe metadata workflow by default:

```text
https://github.com/owner/repository
  -> metadata preview
  -> choose an available action
  -> explicit confirmation
```

The user can then choose an explicit mode:

| Mode | Local catalog | GitHub write | Git Vault |
|---|---:|---:|---:|
| `metadata` | Yes | No | No |
| `track` | Yes | No | Yes |
| `star` | Yes | Yes | Policy-dependent |

Every repository action (`metadata`, `track`, and `star`) requires:

- a connected GitHub account;
- the required provider scope;
- an explicit inline-button or Mini App confirmation;
- an idempotency key;
- an audit record;
- truthful partial-success reporting.

Telegram never receives or stores the GitHub access token. The command path is:

```text
ratatoskr-telegram
  -> authenticated Platform `/v1/gh` gateway
  -> ratatoskr-github preview/action API
  -> component result
  -> durable Telegram message projection
```

If starring succeeds but list filing or backup enrollment fails, the bot reports that partial result and does not undo the successful star.

## Commands and interaction model

The command surface includes or reserves:

```text
/start
/help
/article <url>
/repository <url-or-owner/name>
/status <operation-id>
/search <query>
/recent
/settings
```

Most actions should also work through natural message routing so the user can simply send a supported URL.

Commands create typed interaction intents rather than directly invoking domain clients. Dialogue state is narrow and durable:

- waiting for repository mode;
- waiting for confirmation;
- waiting for note/collection selection;
- waiting for file upload;
- resolving an operation retry.

Every state has an expiry and a safe cancellation path. Long-running domain state remains in Platform operations, not Telegram dialogue rows.

## Inline callbacks

Inline keyboards use opaque 64-character callback tokens. Raw URLs, secrets, complex JSON, or mutable authorization decisions are never embedded directly in callback data.

A callback token resolves server-side to:

```text
dialogue_id
bot_id
telegram_user_id
chat_id
expected_message_id
action
expected_dialogue_version
expires_at
consumed_at
```

Rules:

- callback ownership is verified against bot, Telegram user, chat, and acknowledged message;
- every callback token is one-time and transactionally tied to the expected dialogue version;
- expiry is enforced;
- a replay receives `This action has expired. Please start again.` and never re-executes;
- authorization is rechecked when the action executes;
- consumed tokens cannot be replayed.

## Telegram Mini App

The Mini App provides a richer UI for:

- adding an article and selecting analysis policy;
- choosing tags and local collections;
- adding a GitHub repository;
- choosing `metadata`, `track`, or `star`;
- choosing a native GitHub star list;
- configuring backup policy;
- following operation progress and warnings;
- searching the local archive;
- opening article and repository details;
- managing Telegram notification preferences.

The Mini App frontend is expected to be a separate entrypoint in the shared web-client codebase, while this repository owns Telegram-specific server integration and authentication.

## Mini App authentication

Authentication uses raw Telegram `initData`, validated on the server. Client-parsed `initDataUnsafe` is never trusted.

Planned flow:

```text
Telegram opens Mini App
  -> Mini App sends raw initData to Platform
  -> Platform delegates validation to ratatoskr-telegram
  -> Telegram validates signature, auth_date, and bot context
  -> Telegram maps telegram_user_id to internal user identity
  -> Telegram returns a short-lived signed identity assertion
  -> Platform issues a short-lived Ratatoskr session
```

Security requirements:

- bot token remains only in this service;
- HMAC/signature verification follows the current Telegram contract;
- stale `auth_date` values are rejected according to configured policy;
- assertion audience, issuer, user, nonce, and expiry are explicit;
- replay is prevented;
- the Mini App receives no bot token or provider credential;
- account linking is auditable and revocable.

The Mini App uses the normal HTTPS Edge API for domain work. `sendData` or arbitrary Web App payloads are treated as untrusted input, not as authenticated commands.

## Deep links and intents

Current Bot API deep links carry exactly one opaque interaction token:

```text
https://t.me/<bot>?start=<64-character-token>
```

The corresponding server record may contain:

```text
token
bot_id
telegram_user_id
chat_id
action
operation_id
typed_payload
expires_at
consumed_at
```

Item 8 implements the `operation_status` intent used by capture results. `/start` parses the token
before URL routing, resolves it once under the issuing bot/user/chat scope, and does not mutate the
Platform operation or message binding. Mini App `startapp` authentication remains item 9.

Raw URLs and workflow JSON are not placed in the deep-link parameter. Intents are user-bound,
expiring, and single-use.

## Data ownership

The service owns a `telegram.*` PostgreSQL schema:

```text
telegram_identities
telegram_chats
telegram_updates
telegram_interactions
telegram.dialog_states
telegram.interaction_tokens
telegram_message_bindings
telegram_notification_preferences
telegram_delivery_attempts
telegram_outbox
telegram_inbox
```

Telegram stores interaction and projection state only. Source content, repositories, analyses, and backups remain in the owning bounded context.

### Message bindings

A binding connects a Platform operation or domain result to a Telegram message:

```text
operation_id
chat_id
message_id
projection_kind
last_rendered_version
last_delivery_status
```

This allows the dispatcher to edit progress safely, avoid duplicate final messages, and recover after restarts.

## Update deduplication and ordering

Telegram may retry webhooks, and downstream events are delivered at least once. The service therefore:

- stores processed update IDs;
- uses idempotency keys when creating Platform operations;
- stores event inbox IDs;
- serializes message edits per chat/message binding;
- treats "message not modified" as a successful no-op;
- retries rate limits and transient network failures with bounded backoff;
- prevents an older progress event from overwriting a newer final projection.

Projection versions are monotonic for one operation binding.

## Access control

Initial self-hosted deployments are owner-first. Access can be restricted through an explicit Telegram identity allowlist or approved account-link records.

Rules:

- unauthorized users receive a generic response without system details;
- group/supergroup support is disabled until separately designed;
- commands are bound to the initiating Telegram user and chat;
- callbacks and Mini App intents cannot be transferred to another user;
- privileged GitHub or backup operations recheck Ratatoskr authorization;
- bot administrators are not inferred solely from Telegram chat roles.

## Notifications

User-configurable notification classes are:

- operation completed;
- operation failed;
- analysis ready;
- backup outcome;
- watch triggered;
- archive imported.

`/settings` controls the global switch, a per-class override, UTC quiet hours, and explicit
high-priority bypass for the current authorized private chat. Notifications contain only escaped
title/optional detail from the bounded contract. Raw domain references, recipient identifiers,
conversation history, and provider credentials are never rendered.

## Commands and events

Expected contracts include:

```text
telegram.update.received.v1
telegram.interaction.created.v1
telegram.interaction.expired.v1
telegram.identity.linked.v1
telegram.message_projection.requested.v1
telegram.message_projection.delivered.v1
telegram.delivery.failed.v1
telegram.mini_app.assertion_issued.v1
content.extraction.requested.v1
github.repository.add_requested.v1
platform.operation.progressed.v1
platform.operation.completed.v1
```

Telegram-specific events never carry provider tokens or full raw content when an opaque source or operation reference is sufficient.

## Security invariants

1. Bot token remains inside this service.
2. Webhook requests verify the configured secret token.
3. Telegram update IDs and event IDs are deduplicated.
4. Webhook handlers return promptly and never wait for long domain work.
5. Raw Mini App `initData` is validated server-side; `initDataUnsafe` is not trusted.
6. Deep links and callbacks use opaque, user-bound, expiring tokens.
7. A pasted GitHub URL does not imply consent to star or back up.
8. GitHub and other provider credentials never enter Telegram state.
9. Message projections cannot regress from a final to an older progress state.
10. Private source content is not copied into logs, traces, or callback payloads.
11. A future MTProto connector requires a separate design and consent boundary.

## Observability

Core metrics include:

```text
telegram_webhook_duration
telegram_updates_received
telegram_updates_deduplicated
telegram_unauthorized_updates
telegram_interactions_created
telegram_interaction_token_presentations_total
telegram_dialogue_transitions_total
telegram_interaction_cleanup_rows_total
telegram_mini_app_auth_failures
telegram_delivery_duration
telegram_delivery_retries
telegram_rate_limit_waits
telegram_projection_lag
telegram_notification_events_total
telegram_notification_backlog
telegram_notification_lag
```

Traces correlate Telegram update, interaction, Platform operation, downstream command, result event, and delivered message.

## Non-goals

- Reading arbitrary private Telegram history as a user.
- Running a Telethon/MTProto userbot in the initial service.
- Owning article, GitHub, backup, or Knowledge data.
- Performing extraction or LLM inference in webhook handlers.
- Storing provider credentials other than the Telegram bot secret.
- Treating Mini App client payloads as trusted without server validation.
- Automatically performing GitHub writes from a pasted URL.
- Supporting groups, channels, or multi-user public bots before an explicit access model exists.

## Initial milestones

1. Scaffold the Rust service and define the first-version `telegram` schema.
2. Implement the Bot API client, secure webhook, durable update admission, deduplication, and restart recovery.
3. Add identity/chat binding and owner access control.
4. Implement the dispatcher and operation projections.
5. Add plain URL article submission.
6. Add file/PDF and forwarded-message ingestion.
7. Add GitHub repository preview and confirmed `metadata`, `track`, and `star` flows. (done)
8. Add callback tokens, dialogue state, and opaque deep-link intents. (done)
9. Add Mini App `initData` validation and Platform identity assertions.
10. Add notifications, deployment and recovery runbooks, and workspace integration tests. (done)

## Workspace integration

The workspace `TG-010` changeset pins compatible Contracts, Platform, and Telegram revisions and
owns the executable `integration/run-telegram-notification.sh` composed profile. That profile drives
the item-5 URL flow through this real webhook and dispatcher, Platform capture/outbox and operation
projection, then proves an enabled notification is sent once while an opted-out notification is
suppressed. It also removes and deliberately misconfigures the Platform-owned notification durable
to prove dispatcher readiness fails closed before Platform restores the exact topology.

The evidence is synthetic: the profile generates isolated credentials, uses a fake Bot API, and
creates fresh task-namespaced PostgreSQL and NATS state. It is evidence of repository integration,
not a live Telegram/provider delivery or a deployed-host check. This repository remains
independently buildable and testable with the same synthetic boundary.

## Project status

Plan items 1 through 8 and item 10 of `docs/IMPLEMENTATION_PLAN.md` are implemented: the workspace builds, both binaries run and answer the operator plane, configuration refuses unknown or invalid values, telemetry correlates, and the first-version `telegram` schema applies at startup. The webhook authenticates, bounds, deduplicates, authorizes, and durably processes private owner updates; the dispatcher owns ordered/rate-limited Bot API delivery, truthful operation projections, and preference-gated notification delivery. Article URLs, bounded attachments, and forwarded links submit through Platform without leaking provider or file credentials.

An exact canonical GitHub repository URL now routes before generic article capture. Telegram reads the preview through Platform's authenticated GitHub gateway, renders only GitHub-reported fields and capabilities, and persists the flow in `telegram.dialog_states`; every button is an opaque owner/bot/chat/message/version-bound row in `telegram.interaction_tokens`. Selecting any mode only produces a second confirmation prompt; exactly one live confirmed token may submit under its durable idempotency identity. Replays and stale or foreign presentations converge on the expired-state response without another action. The same registry backs one-time `/start` operation-status intents, and the webhook worker expires stale dialogue state and removes eligible tokens/retention-expired terminal rows in bounded startup and monotonic-interval passes. Mini App authentication, OAuth, and star-list UI remain future work.
