## Context

The webhook currently authenticates every request with the shared webhook secret and processes every authenticated update. Anyone who learns the bot username can submit work and consume resources. This change introduces identity/chat binding and an authorization gate before command flows arrive, keeping the intake contract unchanged.

Facts this change builds on:

- Updates are admitted (row plus dedupe identity inserted) before acknowledgment and claimed by the webhook worker through PostgreSQL; terminal settlement minimizes payloads while retaining dedupe evidence (`crates/persistence/src/updates.rs`).
- Configuration is environment-driven nested tables parsed with `deny_unknown_fields`; violations exit 78 with `&'static str` messages. Validation rule slots V1-V13 are taken; V14 is next (`crates/core/src/config/validate.rs`).
- Development status forbids migrations: a schema change edits root `schema.sql` in place, and test databases are created fresh from that definition.

## Goals / Non-Goals

Goals:

- Persist Telegram identity and chat records with explicit access state (`enabled`/`disabled`).
- Gate every processable update behind an allow-style policy evaluated in the worker.
- Bootstrap exactly one owner from configuration, idempotently.
- Deny silently: no replies, no existence leaks, identifier-free telemetry.

Non-Goals:

- Enrollment or administration flows for additional users (operator plane work, later items).
- Internal user provisioning: `internal_user_id` stays NULL until Platform identity lands (item 9+).
- Group-chat support beyond refusing it.
- Outbound notification of denials.

## Decisions

### D1: The gate lives in the worker claim-and-settle path, not intake

Intake keeps authenticate-parse-persist-acknowledge. Authorization runs where claims happen (`services/webhook/src/intake/worker.rs`). Admission must remain cheap and durable for restart recovery, and a denied update still needs its dedupe evidence retained and its payload minimized - machinery that already lives in settlement.

Alternative rejected: rejecting unauthorized updates at intake. That would fork the admit path, break the uniform "every authenticated update gets a row" recovery invariant, and still need settlement to clean payloads.

### D2: A dedicated terminal state `denied`

Reusing `failed` would imply retryable domain trouble; `unsupported` implies schema shape. Denial is a policy verdict with distinct observability and no retry. Extending the closed UpdateState vocabulary (text column plus CHECK) costs nothing under the no-migration status.

Rejected alternative: settling denied updates as `unsupported`. It would make policy verdicts indistinguishable from payload-shape facts in telemetry and dead-letter handling.

### D3: Owner bootstrap is a required config value plus insert-if-absent at startup

`RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID` is demanded by validation rule V14 for the webhook role only; the dispatcher starts without it. Startup calls an idempotent ensure: insert the owner identity as `enabled` only when absent, never modifying an existing row. A deliberately disabled owner therefore survives restarts. No bootstrap secret lives in the database.

### D4: `internal_user_id` is a nullable, unenforced reference

Telegram-provided IDs are not canonical internal identity per bounded-context rules. The column reserves the binding slot; the Platform identity service arrives in a later plan item and will own enforcement. No foreign key crosses the schema boundary today.

### D5: Chats are created lazily from updates, private-only

Chat rows appear only when a processable update actually mentions the chat, restricted by CHECK to type `'private'` today. Group refusal needs no chat row and creates none. The private-chat-id-equals-user-id convention is not relied upon anywhere.

### D6: Denial is a silent settle with class-only telemetry

After settling `denied`, the worker emits a counter/log record carrying the outcome class (unknown sender, disabled identity, non-private chat) and correlation ids only. No Bot API call is made, no reply is sent, and the three classes are externally indistinguishable.

## Risks / Trade-offs

- [A misconfigured owner id locks the real owner out and enrolls a stranger] -> V14 rejects malformed values before anything binds; fixing the variable and restarting is idempotent; the wrongly enrolled row can be disabled by direct SQL while no operator plane exists (noted in `.env.example`).
- [Setting the new variable against an older binary exits 78 via deny_unknown_fields] -> deployment note in the proposal: the variable ships together with the upgraded binary, never ahead of it.
- [Silent denial complicates debugging] -> outcome-class counters plus correlation ids make denials visible server-side without exposing them to senders.

## Migration Plan

None. Fresh-schema world per development status: edit `schema.sql` in place; databases are recreated from the definition.

## Open Questions

None blocking. Operator flows for enrolling or re-enabling identities are deferred to later plan items; direct SQL remains the documented interim.
