# Ratatoskr Telegram

`ratatoskr-telegram` is the Telegram interaction bounded context for Ratatoskr Next. It provides a Bot API interface and Telegram Mini App authentication for submitting articles, adding or tracking GitHub repositories, following long-running operations, and receiving notifications from a local Ratatoskr deployment.

> **Status:** architecture bootstrap. No bot webhook, dispatcher, command handlers, Mini App authentication, database schema, or deployment configuration is implemented yet.

## Role in Ratatoskr Next

Telegram is a first-class integration rather than a generic adapter hidden inside Platform. It owns Telegram-specific credentials, identities, chats, update deduplication, dialogue state, callback confirmations, deep-link intents, and message projections.

It does **not** own:

- article content or extraction results;
- GitHub repositories, stars, lists, or provider tokens;
- Git mirrors or snapshots;
- summaries, embeddings, or search indexes;
- general Ratatoskr collections;
- user account credentials for unrelated providers.

For domain work, Telegram creates authenticated commands through Platform and renders the resulting operation state back into Telegram messages.

## Planned deployables

```text
ratatoskr-telegram/
├── crates/
│   ├── bot-api/
│   ├── interactions/
│   ├── mini-app-auth/
│   ├── message-projections/
│   └── telegram-infrastructure/
├── services/
│   ├── webhook/
│   └── dispatcher/
└── migrations/
```

### `ratatoskr-telegram-webhook`

The webhook process:

- accepts Telegram Bot API updates;
- verifies the configured webhook secret;
- deduplicates update IDs;
- resolves Telegram identity and access policy;
- parses commands, messages, callbacks, forwards, and files;
- creates an interaction and Platform operation;
- sends or schedules a prompt acknowledgement;
- returns a successful HTTP response without waiting for extraction, LLM, GitHub, or backup work.

### `ratatoskr-telegram-dispatcher`

The dispatcher:

- consumes operation and domain events;
- maintains per-chat ordering and rate limits;
- sends initial, progress, final, and warning messages;
- edits existing status messages where appropriate;
- resolves callback-token results;
- retries transient Bot API failures;
- records Telegram message bindings and notification state.

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
- a PDF or supported document;
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
  -> add to local GitHub catalog
```

The user can then choose an explicit mode:

| Mode | Local catalog | GitHub write | Git Vault |
|---|---:|---:|---:|
| `metadata` | Yes | No | No |
| `track` | Yes | No | Yes |
| `star` | Yes | Yes | Policy-dependent |

External GitHub writes require:

- a connected GitHub account;
- the required provider scope;
- an explicit inline-button or Mini App confirmation;
- an idempotency key;
- an audit record;
- truthful partial-success reporting.

Telegram never receives or stores the GitHub access token. The command path is:

```text
ratatoskr-telegram
  -> Platform operation
  -> github.repository.add_requested.v1
  -> ratatoskr-github
  -> optional vault.target.desired.v1
  -> operation events
  -> Telegram message projection
```

If starring succeeds but list filing or backup enrollment fails, the bot reports that partial result and does not undo the successful star.

## Commands and interaction model

The initial command surface may include:

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

Inline keyboards use short opaque callback tokens. Raw URLs, secrets, complex JSON, or mutable authorization decisions are never embedded directly in callback data.

A callback token resolves server-side to:

```text
interaction_id
user_id
chat_id
action
payload reference
expires_at
consumed_at
```

Rules:

- callback ownership is verified against the Telegram user and chat;
- destructive or external-write actions use one-time tokens;
- expiry is enforced;
- duplicate callback delivery is idempotent;
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

Deep links carry a short opaque intent ID:

```text
https://t.me/<bot>?startapp=<opaque-intent-id>
```

The corresponding server record may contain:

```text
id
user_id
kind
payload_blob_ref
expires_at
consumed_at
```

This supports opening the Mini App directly on:

- article preview;
- repository preview;
- operation status;
- search result;
- confirmation form.

Raw URLs and workflow JSON are not placed in the deep-link parameter. Intents are user-bound, short-lived, and optionally one-time.

## Data ownership

The service owns a `telegram.*` PostgreSQL schema:

```text
telegram_identities
telegram_chats
telegram_updates
telegram_interactions
telegram_dialog_states
telegram_interaction_intents
telegram_callback_tokens
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

User-configurable notifications may include:

- article processing completed;
- GitHub repository added or partially added;
- Git backup degraded;
- restore drill failed;
- provider reauthorization required;
- ChatGPT/Claude export backup is stale;
- archive import completed with missing assets;
- manual operation needs confirmation.

Notifications contain minimal safe text and opaque links. Sensitive conversation or private repository content is not included unless the user explicitly configures that behavior.

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
telegram_callback_replays_blocked
telegram_mini_app_auth_failures
telegram_delivery_duration
telegram_delivery_retries
telegram_rate_limit_waits
telegram_projection_lag
telegram_operation_completion_notifications
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

1. Define Telegram identity, update, interaction, callback, intent, and message-binding schemas.
2. Implement the Bot API client and secret-verified webhook.
3. Add update deduplication and owner access control.
4. Implement plain URL article submission and operation projections.
5. Add GitHub repository `metadata`, `track`, and confirmed `star` flows.
6. Add file/PDF and forwarded-message ingestion.
7. Implement dispatcher retries, ordering, and progress-message editing.
8. Add Mini App `initData` validation and Platform identity assertions.
9. Add opaque deep-link intents and notification preferences.
10. Add integration tests against Platform, Extractor, GitHub, Vault, Knowledge, and the Mini App frontend.

## Workspace integration

`ratatoskr-workspace` pins Telegram with compatible Platform, Contracts, Extractor, Knowledge, GitHub, Vault, and web/Mini App commits. The repository remains independently buildable and testable using recorded Bot API fixtures and a mock Telegram server.

## Project status

This README defines the intended Telegram Bot API and Mini App integration architecture. No webhook, bot client, dispatcher, authentication validator, persistence layer, or command implementation exists yet.
