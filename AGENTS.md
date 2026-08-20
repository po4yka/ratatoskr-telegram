# Ratatoskr Telegram Agent Instructions

## Scope

These instructions apply to the `ratatoskr-telegram` repository and its planned runtime roles:

- webhook receiver;
- event-driven message dispatcher;
- Telegram identity/Mini App authentication support;
- bot command, callback, and dialogue-state handling.

The Telegram Mini App frontend may live in `ratatoskr-web`; this repository owns the Telegram backend boundary and authentication/projection semantics.

## Repository mission

The service lets a user interact with Ratatoskr through a Telegram bot and Mini App, including:

- submitting article URLs, text, PDFs, and forwarded messages;
- adding GitHub repositories in `metadata`, `track`, or `star` mode;
- selecting GitHub star lists and backup policy through explicit flows;
- opening a Mini App for richer forms, search, and status;
- receiving progress, completion, partial-success, and failure messages;
- receiving authorized notifications.

Telegram owns interaction state. It does not own articles, GitHub repositories, backups, or analyses.

## Current phase

The repository is in architecture bootstrap. Do not assume Rust crates, Bot API client, webhook, dispatcher, database schema, Mini App validator, commands, or CI checks exist unless they are present in the checkout.

When creating initial implementation:

- separate webhook intake from outbound dispatch;
- acknowledge Telegram quickly and perform domain work asynchronously;
- persist deduplication and message bindings;
- keep provider/domain credentials out of this service;
- use opaque server-side intents for callbacks and Mini App deep links.

### Development status

Ratatoskr is in development. No database holds data that has to survive a schema change. While this
status holds, these rules are binding, and they override anything else in this repository that
plans otherwise, including the rest of this file:

- **One version only.** The API, the database, and the contracts keep their first version. Do not
  add a `v2` or a later major version, and do not add version negotiation, deprecation windows, or
  parallel-major routing.
- **No database migrations.** Do not add a migration file, and do not add migration tooling. A
  schema change edits the current schema definition in place, and a test database is created from
  that definition.
- **The product is `Ratatoskr`.** It is not "Ratatoskr Next". Do not write that name in code,
  documentation, identifiers, comments, or commit messages.

Only the repository owner changes this status. Ask before you write anything these rules forbid.

## How a change starts

Every non-trivial change begins as an OpenSpec change rather than as an edit. In your assistant that
is `/opsx:propose <what you want to build>`, or `/opsx:explore` first when the shape is not clear
yet. The command writes `openspec/changes/<id>/` holding a proposal, the spec deltas, a design and a
task list, and you read that plan before any code is written. `/opsx:apply` builds it and
`/opsx:archive` folds the deltas into `openspec/specs/`.

`openspec/specs/` holds the behaviour that is true today, and it starts empty on purpose. A spec here
grows from a change that needed it. Do NOT convert `docs/REQUIREMENTS.md`, `docs/INTERFACES.md`,
`docs/DOMAIN.md` or `docs/DATA_MODEL.md` into specs in bulk. Those documents stay where they are, as
material an exploration reads. A spec set produced by bulk conversion is large, stale on the day it
lands, and trusted by nobody.

Behaviour that more than one repository can see — the shape of a contract, the meaning of a field, the
order in which repositories must receive a change — belongs in the `ratatoskr-workspace` store, not
here. `openspec/config.yaml` references it, so `openspec instructions` in this repository lists the
store's specs with the exact command that fetches one. Cite that spec from a local proposal instead
of restating it.

### Tests come first

The task list carries one pair per behaviour. The first task adds a test that fails. The second makes
it pass. Never one task that does both.

- Run the new test before you write the implementation, and confirm it fails for the reason the task
  states — not for a compile error or a typo.
- A refactor task comes after the tests are green. It adds no test and changes no behaviour.
- A task that cannot start from a failing test says why in one line. Configuration, documentation and
  generated files are the usual reasons.
- Do not tick a task whose test has not been run.

Nothing can check the order in which the two were written. What CI does check is
`openspec validate --archived`, which fails when a change was archived with a task left unticked, and
the step in `fleet.yml` that fails when a repository holds a manifest and a `ci.yml` that never runs
a test. `ratatoskr-workspace/docs/QUALITY_GATES.md` states that limit rather than implying it is
covered.

## Sources of truth

Use this order:

1. active task/changeset and accepted ADRs;
2. `README.md`;
3. Telegram/platform/event contracts from `ratatoskr-contracts`;
4. Telegram Bot API update data after authenticity/access validation;
5. persisted interaction/message projection state;
6. implementation details.

Telegram-provided user/chat IDs identify Telegram principals; they are not Ratatoskr's canonical internal user identity.

## Hard bounded-context rules

### Telegram service owns

- bot configuration and Bot API credential;
- webhook validation and update deduplication;
- Telegram identity-to-internal-user binding;
- known chats and access policy;
- command/callback/dialogue state;
- opaque callback and Mini App intent records;
- Telegram message bindings for Ratatoskr operations;
- outbound queue, ordering, retry, and rate-limit state;
- notification preferences specific to Telegram;
- Mini App `initData` validation and short-lived identity assertion;
- Telegram-specific outbox/inbox records.

### Telegram service does not own

- Platform sessions other than Telegram-issued identity assertions;
- article/document bodies or extraction state;
- Knowledge analyses/embeddings/search index;
- GitHub repository/star/list/credential state;
- Git mirrors, snapshots, or retention;
- X/Instagram/Threads provider state;
- ChatGPT/Claude archives;
- general user collections or client UI state;
- provider credentials other than the Telegram bot token.

Never call provider APIs directly. Send commands through Platform/contracts to the owning service.

## Bot API boundary

Use the official Telegram Bot API for the supported bot/Mini App workflows.

Do not add Telethon, TDLib, or user-account MTProto sessions merely to implement bot features.

A future requirement to read personal dialogs/channel history as the user is a separate connector with different credentials, consent, security, data ownership, and ADR. It must not be hidden inside this service.

## Runtime separation

### Webhook receiver

The webhook role should:

- expose the configured HTTPS endpoint;
- validate the Telegram webhook secret/header;
- enforce request size/content type limits;
- parse and schema-check the update;
- deduplicate by bot/update identity;
- persist the accepted interaction/update and enqueue processing;
- return a successful acknowledgment quickly.

It must not wait for article extraction, GitHub mutation, backup, or LLM work.

### Dispatcher

The dispatcher should:

- consume operation/domain events and Telegram outbound jobs;
- project updates into edits/new messages;
- preserve per-chat/message ordering where required;
- apply Bot API rate limits and backoff;
- retry eligible failures idempotently;
- handle message-not-modified/deleted/chat-blocked/forbidden outcomes explicitly;
- update message binding state only after provider acknowledgment.

The two roles may share code or one binary with modes, but their runtime responsibilities remain distinct.

## Webhook security

- Configure and verify a high-entropy webhook secret.
- Compare secret values safely.
- Reject missing/invalid secrets before parsing expensive payloads.
- Restrict request methods and content types.
- Enforce body and nesting limits.
- Do not use source IP allowlisting as the sole authentication mechanism.
- Terminate TLS through an explicitly trusted deployment path.
- Never log the bot token, webhook secret, raw Mini App auth material, or private message bodies by default.
- Record request/update/correlation IDs and user-safe failure class.

Changing webhook registration is an operational write and should be explicit/auditable.

## Update deduplication and processing

Telegram can redeliver updates or the service can retry after uncertain acknowledgment.

- Persist a unique key based on bot identity and `update_id`/equivalent.
- Insert/deduplicate transactionally before domain side effects.
- Make command/callback handlers idempotent.
- Preserve the original update as restricted raw evidence only if policy requires it; otherwise retain minimized parsed fields.
- Distinguish unsupported update types from malformed/unauthorized updates.
- Do not acknowledge as successfully processed merely because the webhook accepted it; interaction processing has its own state.
- Bound retries and route permanent failures to diagnostics/dead-letter handling.

## Access control and identity binding

A Telegram bot must not become public merely because anyone can message it.

- Maintain an explicit allow/enrollment policy for Telegram users and chats.
- Map Telegram user identity to an internal Ratatoskr user UUID through a verified binding flow.
- Use chat type and actor identity separately; a group chat ID is not a user identity.
- Re-check authorization for every command, callback, Mini App login, retry, and result view.
- Do not trust forwarded sender fields as authenticated actor identity.
- In group contexts, define privacy and command-addressing rules explicitly before support.
- Deny unknown/disabled users with a minimal non-sensitive response.
- Audit bindings, revocations, and privileged actions.

Provider IDs never become the primary identity in Platform/domain contracts.

## Command model

Commands and natural URL messages are adapters to explicit Ratatoskr operations.

Possible commands may include:

```text
/start
/help
/article <url>
/repository <url>
/status <operation>
/search <query>
/settings
```

Rules:

- parse commands deterministically;
- validate length, URL scheme, attachment count, and supported input type;
- do not infer high-impact external writes from ambiguous free text;
- use confirmation buttons/forms for material GitHub/provider writes;
- keep multi-step dialogue state persisted with expiry;
- reject stale or replayed dialogue transitions;
- allow cancellation and recovery;
- avoid storing entire private message histories as dialogue state.

A bare GitHub URL may safely default to a metadata preview/catalog action, not an automatic provider star or backup deletion.

## Article and document flow

Supported input may include:

- a URL message;
- `/article <url>`;
- forwarded channel/message text containing URLs;
- multiple explicit links within configured limits;
- PDF/supported document attachment;
- explicitly saved forwarded text as a note/source.

Flow:

```text
Telegram update
  -> validate/deduplicate/authorize
  -> create Platform operation/command
  -> receive operation progress events
  -> edit/send Telegram projection
```

Rules:

- do not fetch or scrape the article in Telegram service;
- do not call Knowledge/LLM inline;
- preserve Telegram capture provenance and original message/attachment references according to privacy policy;
- download Bot API files only when required, with size/type/time limits, then stream/upload through Platform;
- treat filenames, captions, URLs, PDFs, and forwarded text as untrusted;
- do not execute or render active document content;
- show partial/failure status truthfully;
- avoid leaking private article/note contents in group chats or lock-screen notifications.

## Telegram file handling

When receiving Telegram documents/media:

- enforce allowlisted types and configured file/total size limits before download where metadata allows;
- retrieve file metadata through Bot API with bounded retries;
- stream bytes instead of loading entire files into memory;
- compute hash while streaming where useful;
- upload through Platform using an idempotency key;
- keep Bot API file identifiers separate from durable BlobStore identity;
- validate MIME/content evidence and sanitize filenames;
- clean only service-owned temporary files;
- do not persist bot download URLs/tokens in logs;
- do not treat a thumbnail as the original asset.

A download/upload success is not extraction/analysis success.

## GitHub repository flow

A GitHub repository URL should produce a metadata preview and explicit available actions:

```text
metadata  -> catalog only
track     -> catalog + desired backup
star      -> provider star, optional list filing and backup
```

Rules:

- Telegram never receives/stores the user's GitHub token;
- send the requested mode through Platform to `ratatoskr-github`;
- `star` and native list changes require connected account, scope, explicit confirmation, idempotency, and audit;
- backup choices are desired policy for Vault, not Git commands;
- do not run Git or call GitHub APIs directly;
- return step-level partial success, e.g. star succeeded/list filing failed/backup accepted;
- do not automatically retry a successful provider mutation;
- do not infer backup health from command acceptance;
- destructive unstar/unenroll/delete operations require an even clearer explicit flow.

Inline callback labels are presentation; server-side intent records are authority.

## Callback queries

Telegram callback payloads are small and replayable. Do not embed trusted business state directly in them.

Use opaque single-purpose callback tokens referencing a server-side record containing:

- internal user/chat binding;
- intended action and version;
- resource/operation reference;
- expected current state;
- expiry;
- one-time/replay policy;
- confirmation/audit context.

Rules:

- verify callback actor/chat binding;
- expire tokens quickly according to action risk;
- consume one-time tokens transactionally;
- reject stale/replayed/foreign callbacks;
- never place provider tokens, raw URLs with secrets, full JSON, or mutable policy state in callback data;
- answer callback queries promptly even when domain work continues asynchronously;
- use a new intent for retries or changed options.

## Deep links and Mini App intents

Use an opaque intent ID:

```text
https://t.me/<bot>?startapp=<opaque_intent_id>
```

Server-side intent records should include:

- owner internal user and Telegram identity;
- intent type;
- resource/payload reference;
- expiry;
- consumed/reuse policy;
- creation/causation metadata.

Do not embed raw URLs, provider IDs, secrets, notes, or full operation payloads in `startapp`.

Forwarded/deep-link reuse must not let another Telegram user access the original user's data.

## Mini App authentication

Raw Telegram `initData` is sent to the backend for validation. `initDataUnsafe` is display convenience only and never trusted.

Validation requirements:

- verify the Telegram signature/HMAC according to the supported protocol;
- use the bot token only inside this service;
- parse the validated payload with strict limits;
- verify `auth_date` freshness/replay window;
- bind Telegram user/chat/start parameter to the expected server-side intent where applicable;
- verify the Telegram identity is linked/authorized for the internal user;
- issue a short-lived signed Telegram identity assertion to Platform;
- let Platform issue the actual Ratatoskr session;
- prevent assertion reuse/audience confusion;
- record validation/auth failures without raw secret material.

Mini App domain APIs use normal authenticated HTTPS to Platform. `sendData`/web-app data is untrusted user input and must not authorize domain writes.

## Mini App boundary

The Mini App frontend may provide richer UI for:

- article capture and collection selection;
- GitHub repository mode/list/backup policy;
- operation status and errors;
- search and detail views;
- notification preferences.

Rules:

- Telegram service owns identity validation and intent resolution, not general frontend domain state;
- Mini App calls Platform public APIs, not Telegram/internal services for domain operations;
- capability discovery controls available features;
- Telegram theme/viewport/back button are presentation concerns;
- no provider credentials enter JavaScript;
- validate every deep link/intent/session independently;
- do not duplicate web business logic if it can be shared safely in `ratatoskr-web` packages.

## Dialogue state

Persist only the state required for multi-step interactions:

- dialogue ID/version;
- internal user/chat;
- current step;
- selected safe options/resource references;
- expiry;
- last processed update/callback;
- cancellation/completion state.

Rules:

- transitions require expected current version/step;
- duplicate updates are harmless;
- expiry/cancel cleans state without deleting domain operations;
- do not place provider tokens or full article/chat bodies in dialogue records;
- resuming after service restart is deterministic;
- only one writer/transition wins for a dialogue version;
- high-impact actions require a final explicit confirmation state.

## Message projection and bindings

Bind Telegram messages to Ratatoskr operations/resources:

```text
telegram chat/message
  <-> internal operation/resource
  + last rendered projection version
```

Rules:

- operation events may duplicate or arrive out of order;
- apply sequence/version monotonicity;
- edit the existing progress message when appropriate;
- create a new message only when edit is impossible or UX requires it;
- handle deleted messages and expired edit windows;
- do not regress a terminal projection;
- avoid rendering full sensitive content unnecessarily;
- validate Markdown/HTML escaping for all dynamic text;
- buttons use opaque intents/tokens;
- store provider message IDs only after successful Bot API response.

## Progress UX

Progress may show stages such as:

```text
accepted
retrieving/resolving
extracting
analyzing
indexing/backing up
completed/partial/failed
```

Rules:

- do not fake fine-grained stages absent from backend events;
- throttle/coalesce edits to respect rate limits and reduce noise;
- show actionable stable errors;
- distinguish retryable failure, permanent validation, reauth-required, and partial success;
- include links/buttons only after authorized resource references exist;
- never declare backup verified before Vault verification/restore evidence.

## Outbound rate limits and ordering

- Apply global, per-chat, and method-aware rate control.
- Serialize or sequence conflicting edits for the same message.
- Use a durable outbound queue.
- Honor provider retry responses.
- Back off with jitter for eligible failures.
- Treat blocked bot, forbidden chat, message deleted/not found, invalid markup, and message-not-modified as distinct outcomes.
- Bound retries and dead-letter permanent failures.
- Prevent one noisy chat from starving all others.
- Coalesce superseded progress updates.

Do not use unbounded concurrent `sendMessage`/`editMessage` calls.

## Notifications

Telegram notification preferences may include:

- operation completion/failure;
- backup verification problems;
- connector reauthorization;
- archive staleness/import result;
- watch/digest events explicitly enabled by product scope.

Rules:

- default to minimal private content;
- respect per-user/chat preference and quiet-time policy when implemented;
- do not send to an unverified/new chat because IDs look similar;
- include an authorized opaque deep link rather than raw sensitive payload;
- deduplicate notifications by event/notification ID;
- make opt-out/revocation clear;
- do not build a generic automation engine inside Telegram.

## Persistence and migrations

Telegram writes only its owned schema.

Conceptual data includes:

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
telegram_outbound_jobs
telegram_outbox
telegram_inbox
```

Rules:

- no cross-schema writes or foreign keys;
- uniqueness enforces update/callback/message/idempotency identities;
- bot tokens/secrets are not stored in general tables;
- domain resource data remains a reference/projection, not copied ownership;
- migrations preserve dedupe, binding, audit, and replay safety;
- retention minimizes raw Telegram message content.

## Commands and events

Representative messages include:

```text
telegram.interaction.accepted.v1
telegram.notification.requested.v1
content.capture.requested.v1
github.repository.add_requested.v1
platform.operation.progressed.v1
platform.operation.completed.v1
social.connection.reauth_required.v1
```

Use canonical contracts, transactional outbox, inbox deduplication, correlation/causation IDs, and at-least-once-safe handlers.

Events/commands contain stable references, not provider credentials or full private messages.

## Security and privacy

- Keep bot token and webhook secret in the service secret store only.
- Never expose bot token to Mini App frontend or Platform.
- Validate all updates, callbacks, deep links, `initData`, and actor/chat/resource ownership.
- Escape/sanitize Telegram Markdown/HTML and Mini App content.
- Treat messages, captions, usernames, filenames, URLs, and attachments as untrusted.
- Do not log raw message bodies, files, notes, `initData`, secrets, callback payloads, or provider errors by default.
- Redact user-facing errors.
- Use least-privilege database/network access.
- Audit identity binding, provider-write confirmations, notification changes, and Mini App sessions.
- Do not leak one user's operation/result into another chat or forwarded deep link.

## Observability

Required telemetry should cover:

- webhook accepted/rejected/deduplicated updates;
- update type and processing state without content;
- interaction/dialogue/callback outcomes;
- Mini App validation/session assertion failures by safe class;
- outbound queue depth, rate limiting, retries, and Bot API failure classes;
- operation projection latency and coalesced edits;
- file download/upload bytes and limits;
- identity/reauth state without private identifiers in metric labels;
- outbox/inbox lag and duplicates;
- correlation, update, interaction, operation, and message-binding IDs in non-sensitive form.

Avoid usernames, chat titles, message text, URLs, and filenames as ordinary metric labels.

## Testing expectations

When implementation exists, include applicable tests for:

- webhook secret validation, body limits, and malformed updates;
- update deduplication and uncertain acknowledgment;
- allowlist/enrollment and identity binding;
- command parsing, URL/file limits, and forwarded-message behavior;
- callback actor binding, expiry, one-time consumption, and replay;
- opaque deep-link/Mini App intent ownership and forwarding attacks;
- `initData` signature/HMAC, freshness, malformed fields, audience, and replay;
- dialogue state optimistic/versioned transitions;
- article/file upload delegation and cleanup;
- GitHub metadata/track/star confirmation and partial outcomes;
- operation duplicate/out-of-order projection;
- Markdown/HTML injection escaping;
- outbound per-chat ordering, edit coalescing, rate-limit, blocked/deleted-message outcomes;
- notification deduplication/preferences;
- outbox/inbox replay and migrations.

Use synthetic updates and fake Bot API servers. Never test normal CI with the production bot token, real chats, personal messages, or private files.

## Cross-repository change rules

Use a workspace changeset when changing:

- Platform auth/capture/operation APIs;
- Telegram/Mini App event contracts;
- GitHub repository modes/partial results;
- article/file upload contracts;
- web Mini App entrypoint/deep links;
- notification events;
- deployment secrets/webhook endpoints.

List producer/consumer compatibility, rollout, rollback, old-bot/old-Mini-App behavior, security/privacy, and user-visible interaction impact.

## Git and PR workflow

- State affected surfaces: webhook, handler, dialogue, callback, Mini App auth, dispatcher, file handling, notifications, deployment.
- Keep bot-token/webhook/auth changes separate from unrelated UX refactors when possible.
- Include synthetic Bot API fixtures and replay/ordering/security tests.
- Document commands, callbacks, permissions, external writes, deep links, and data retention.
- Do not add provider credentials, Git, scraping, LLM, or domain database logic.
- Do not add MTProto/userbot functionality without a separate ADR/repository boundary.
- Do not commit bot tokens, webhook secrets, real updates, chat IDs, message text, files, or screenshots with private data.
- Update README/ADRs when interaction/auth/ownership changes.

## Completion criteria

A task is complete only when:

- responsibility belongs to Telegram interaction/projection;
- webhook validates authenticity, deduplicates, persists, and acknowledges quickly;
- long domain work remains asynchronous and delegated;
- Telegram identities map safely to internal users/chats;
- callbacks/deep links use opaque expiring server-side intents;
- Mini App `initData` is validated server-side and bot token never reaches the client;
- article/file/GitHub flows preserve explicit intent, limits, confirmation, idempotency, and partial results;
- outbound queue enforces ordering/rate limits and handles duplicate/out-of-order events;
- dynamic markup/content is escaped and private data stays out of logs/notifications;
- no MTProto/userbot/provider-domain responsibility is introduced;
- relevant webhook, auth, callback, dialogue, projection, and Bot API tests pass;
- contracts, migrations, telemetry, deployment, and cross-repository rollout are documented.
