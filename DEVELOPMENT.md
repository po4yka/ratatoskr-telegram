# Developing Ratatoskr Telegram

> Status: Implemented for plan items 1 through 6; items 7 through 10 are Proposed.
> Last reviewed: 2026-08-26

## Current stage

Plan items 1 through 3 of `docs/IMPLEMENTATION_PLAN.md` are implemented and the commands marked **real**
below are real. The Cargo workspace, its pinned toolchain and its committed `Cargo.lock`; the
`ratatoskr-telegram-core`, `ratatoskr-telegram-telemetry`, `ratatoskr-telegram-http` and
`ratatoskr-telegram-persistence` library crates; the `ratatoskr-telegram-webhook` and
`ratatoskr-telegram-dispatcher` binaries; typed configuration loaded from `RATATOSKR__` environment
variables with unknown keys refused; the typed error hierarchy with its internal arm; the `tracing`
subscriber with optional OTLP span export and real trace ids without a collector; liveness,
readiness, Prometheus metrics and version on a per-role operator listener (webhook 9467, dispatcher
9468); SIGTERM draining; the first-version `telegram` schema in `schema.sql`, applied at startup to
a configured database; and the CI gate in `.github/workflows/ci.yml`.

Plan item 2 added the `ratatoskr-telegram-bot-api` crate — the typed Bot API client over `teloxide`
(`get_me`, `set_webhook`, `send_message`, `edit_message_text`, `answer_callback_query`,
`send_chat_action`) — and the secure webhook intake: the public listener on 9469 that verifies the
secret header in constant time before reading anything, enforces method/content-type/body-size
limits, parses updates against the Bot API schema, and persists the payload and deduplication key
`(bot_id, update_id)` in `telegram.updates` before acknowledgment. A worker claims pending work from
PostgreSQL, including after restart; the bounded in-process channel is only a wake-up and backpressure
hint. Terminal settlement removes the processable payload while keeping deduplication evidence.
Malformed payloads are acked and logged, never retried into a storm.

Plan item 3 added identity/chat binding and the owner access gate. `telegram.identities` and
`telegram.chats` persist who the deployment admits (closed `enabled`/`disabled` vocabularies,
private-only chats); the worker resolves sender and chat before any domain action and settles
refusals as a new terminal `denied` state — silently, with class-only telemetry, externally
indistinguishable across the three refusal classes. Startup seeds exactly one enabled owner row
from `RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID` (validation rule V14, required for the webhook
role only); bootstrap is insert-if-absent, so an operator-disabled owner survives restarts.

Plan item 5 added the first product slice. The webhook worker parses authorized private-message
text into bare-URL or `/summarize` capture intents, authenticates to Platform by exchanging
short-lived Ed25519-signed assertions (cached per sender until near expiry), and submits captures
under deterministic idempotency keys - resending one link converges on Platform's original
operation, and acceptance pre-creates the message binding, writes an opaque deep-link intent, and
enqueues exactly one HTML acknowledgment. A dispatcher follower consumes Platform's per-operation
SSE stream into the existing projection seam (dedupe by frame id, `Last-Event-ID` resume, stop at
terminal), and terminal renders add the fallback hyperlink plus a Mini App button backed by the
intent record. Platform outages settle updates as failed after bounded retries; nothing is sent
for an unaccepted capture.

Plan item 6 extends capture intake without moving extraction into Telegram: forwarded text or
captions contribute their first external link together with bounded forward provenance, while PDFs
and photos are resolved through the Bot API, streamed into this service's content-addressed blob
store, hashed as SHA-256, and submitted to Platform as opaque `BlobRef` captures. The only
accepted document type is `application/pdf`; video, voice, audio, and other document types receive
one explicit unsupported reply. Declared and streamed byte limits are enforced before and during
download, and a URL-less terminal render describes the attachment without inventing a link.

Not present yet, in plan order: GitHub flows (item 7),
callback tokens and dialogue state including retry buttons (item 8), Mini App initData validation
(item 9), notifications and workspace integration (item 10). No test contacts Telegram or a live
Platform: both clients run against local harness servers with synthetic bodies.

The database is REQUIRED for both roles since item 4: intake writes update deduplication through
the pool, and the dispatcher delivers every send and edit through its durable outbound queue there,
so either role that cannot reach its database refuses to start.

## The dispatcher's delivery pipeline

The dispatcher owns everything Telegram sends. Every `sendMessage`/`editMessageText` is first a row
in `telegram.outbound_jobs` (`ready`), claimed strictly FIFO per chat with one job in flight per
chat; a global token bucket and a per-chat minimum interval gate each wire call; `Retry-After`
reschedules authoritatively and cools the chat; transient failures retry with capped jittered
backoff to a bounded attempt count and then dead-letter as `failed_permanent`; permanent Bot API
answers settle immediately, and a permanent edit failure unbinds so the next revision sends fresh.
Edits carry revisions: stale ones are superseded before any wire call and identical re-renders are
no-ops via content hash, with the Bot API's `message is not modified` counted as success.
`telegram.message_bindings` anchors one live chat message per Platform operation;
`platform.operation.progressed.v1` snapshots consumed over an event seam render into that message
through HTML-escaped, status-led text throttled by durable reschedule arithmetic, with terminal
states applied exactly once per binding. Limits live under `RATATOSKR__DISPATCHER__*` (rule V15):
`GLOBAL_MESSAGES_PER_SECOND`, `PER_CHAT_MIN_INTERVAL_MS`, `RENDER_INTERVAL_SECS`, `MAX_ATTEMPTS`,
`BACKOFF_BASE_SECS`, `BACKOFF_CAP_SECS`, `JITTER_FRACTION_MILLI`, `LEASE_TTL_SECS`,
`POLL_IDLE_MS`. The NATS transport for operation events arrives with workspace integration; today
the consumer is fed through its in-process seam.

## Toolchain

Rust/Tokio/Axum/SQLx, pinned by `rust-toolchain.toml`. The Telegram Bot API client is `teloxide`
(pinned in the workspace manifest, rustls TLS); only `ratatoskr-telegram-bot-api` may depend on it,
which keeps the token and the URL shapes that carry it in one boundary. PostgreSQL 17 locally through
`compose.yaml`. There are no migrations by development-status rule: one schema file, applied fresh,
edited in place.

## Code size limits

The limits live in `clippy.toml` beside the workspace `Cargo.toml`: functions of at most 100 lines,
signatures of at most 7 arguments, block nesting of at most 5, and 850 lines per tracked `.rs` file
enforced by a gate step because no Rust lint counts file length. The numbers are the fleet's
(`ratatoskr-workspace/docs/QUALITY_GATES.md`), so this tree starts on the same ratchet as the other
Rust workspaces instead of choosing looser ones for code that does not exist yet. An exception is
taken at the site with `#[expect(..., reason = "...")]`, never by raising a number here.

## Command families

### Rust — also the CI gate, in this order

```bash
cargo fetch --locked
cargo deny check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
git ls-files -z "*.rs" | xargs -0 -r wc -l | awk '$2 != "total" && $1 > 850 { print; bad = 1 } END { exit bad }'
cargo build --workspace --locked
cargo test --workspace --locked
cargo build --workspace --locked --release
```

The debug build before `cargo test` is not redundant. `services/webhook/tests/boot.rs` executes
`ratatoskr-telegram-dispatcher` as a child process, and `cargo test` builds the binary of the
package under test only — it never produces a sibling package's plain binary. Skip this step and
the boot test fails on any clean checkout.

This list and the step list in `.github/workflows/ci.yml` are the same list. If they drift, this
document is wrong. That is asserted by a step in the workflow rather than left to whoever edits one
of the two files.

`cargo deny check` is in the list because nothing else in the gate reads RustSec. `deny.toml` also
pins the dependency-source policy, including `required-git-spec = "rev"`, which turns the prose rule
about branches and tags not pinning into an exit code.

### Test — real

```bash
# everything, against the local database from compose.yaml
docker compose up -d
cargo build --workspace --locked
cargo test --workspace --locked

# one package while iterating
cargo test -p ratatoskr-telegram-core

# the boot tests run the real binaries and need them built first
cargo test -p ratatoskr-telegram-webhook --test boot
```

The suite creates disposable databases named after each test and drops them afterwards.
`TELEGRAM_TEST_DATABASE_URL` names the server they are created on; unset, it defaults to
`postgres://telegram:telegram@127.0.0.1:5432/telegram`, which is what `compose.yaml` serves. A test
that cannot reach a server FAILS rather than skips: a suite that silently passes without a database
proves nothing.

### Local run — real

```bash
docker compose up -d          # the local PostgreSQL the integration tests use

# dispatcher role, operator plane on 9468; needs its database and bot token before it will start
RATATOSKR__TELEMETRY__LOG_FORMAT=pretty \
RATATOSKR__DATABASE__URL=postgres://telegram:telegram@127.0.0.1:5432/telegram \
RATATOSKR__BOT_API__TOKEN='123456:your-bot-token' \
cargo run -p ratatoskr-telegram-dispatcher

# webhook role: needs its intake configuration before it will start (rule V13)
RATATOSKR__TELEMETRY__LOG_FORMAT=pretty \
RATATOSKR__DATABASE__URL=postgres://telegram:telegram@127.0.0.1:5432/telegram \
RATATOSKR__BOT_API__TOKEN='123456:your-bot-token' \
RATATOSKR__WEBHOOK__SECRET_TOKEN='at-least-16-chars-of-entropy' \
RATATOSKR__INGESTION__BLOB_ROOT="$PWD/.local/telegram-blobs" \
cargo run -p ratatoskr-telegram-webhook
# admin plane on 9467, public intake on 127.0.0.1:9469. Startup calls getMe once to learn the bot
# identity deduplication keys on, so the token must work — point RATATOSKR__BOT_API__BASE_URL at a
# loopback harness for an offline play.

curl -s http://127.0.0.1:9467/health/live
curl -s http://127.0.0.1:9467/health/ready
curl -s http://127.0.0.1:9467/metrics | head
curl -s http://127.0.0.1:9467/version

# deliver a synthetic update through the whole admission path
curl -si http://127.0.0.1:9469/webhook \
  -X POST \
  -H "X-Telegram-Bot-Api-Secret-Token: $RATATOSKR__WEBHOOK__SECRET_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"update_id": 1, "message": {"message_id": 1, "date": 1760000000,
       "chat": {"id": 900700601, "type": "private"}, "text": "https://example.test/a"}}'
# 200 = accepted and queued; repeat it and it is still 200 but dropped as a duplicate.

# validate a configuration without starting anything (exit 0 or 78)
cargo run -p ratatoskr-telegram-webhook -- check-config

# stop cleanly; readiness flips to 503 during the drain while the listener still answers
kill -TERM <pid>
```

Both roles demand their database URL **and** the `RATATOSKR__PLATFORM__` section since item 5:
`BASE_URL` (default `http://127.0.0.1:9463`, https off loopback only), `TIMEOUT_SECONDS`,
`AUDIENCE`, and `ASSERTION_SIGNING_KEY` (64 hex chars = one Ed25519 seed; SECRET). Either role
refuses to start when any of these is missing, beside every other requirement the report names. The dispatcher's outbound
workers start after database preparation and drain in-flight delivery on shutdown; a job left
`sending` by a crashed process is reclaimed after `RATATOSKR__DISPATCHER__LEASE_TTL_SECS`.

The webhook attachment path also owns `RATATOSKR__INGESTION__BLOB_ROOT`: an absolute, durable
directory for Telegram's staged bytes before an opaque BlobRef handoff. Its default is
`/var/lib/ratatoskr-telegram/blobs`; mount that path in production, or set an explicit durable
local directory for development as above. A filesystem path is never sent to Platform, logged as
capture provenance, or rendered to a user.

`RATATOSKR__INGESTION__MAX_ATTACHMENT_BYTES` defaults to 18 MiB and accepts only positive values
up to 20 MiB. It bounds both Telegram's declared file size and the bytes actually streamed, so a
provider response that exceeds its declared size cannot publish a partial blob.

Registering the webhook with Telegram (`setWebhook`) remains an explicit operational write done
outside this process; the client method exists for the tooling that will own it.

### Schema — real

```bash
# `schema.sql` is embedded in the binary by `include_str!`, so there is no separate apply step in a
# deployment: a role applies it at startup, under a PostgreSQL advisory lock, which is what makes a
# restart overlapping the previous process's grace window safe. This raw form is for a database that
# has no schema yet; the file opens with a bare `create schema`, so a second run fails on it.
psql "$TELEGRAM_TEST_DATABASE_URL" -f schema.sql
```

A schema change edits `schema.sql` in place. There is no migration ledger and no second version;
no database holds data that has to survive a schema change. Tables arrive with the features that
own their first writer.

## Workflow

1. Authenticate transport and deduplicate every update before side effects (items 2+).
2. Acknowledge quickly; process durable interactions asynchronously.
3. Keep Telegram identity/interaction/message projection state separate from article, GitHub,
   Vault, and Knowledge authority.
4. Use short opaque callback/deep-link tokens and validate Mini App identity server-side.
5. Test replay, ordering, callback expiry, rate limits, partial domain results, edits/deletes,
   restart, and reauthorization as those items land.

CI uses synthetic updates and synthetic tokens only. The real bot token reaches a process only
through `RATATOSKR__BOT_API__TOKEN`, is a `SecretString` from load to request path, and appears in
no test, fixture, log line or error rendering.

## What a clone needs before you plan a change

A change is planned with OpenSpec, which is a CLI a clone installs for itself. Use the version
`.github/workflows/openspec.yml` pins, so your terminal and the gate answer the same:

```bash
npm install --global @fission-ai/openspec@1.10.0
```

Cross-repository behaviour lives in a store, and registering one is per-machine state that no
repository can turn on for you — the same kind of step as `git config core.hooksPath .githooks`:

```bash
git clone git@github.com:po4yka/ratatoskr-workspace.git <path>
openspec store register <path> --id ratatoskr-workspace
```

`openspec doctor` reports whether both are in place.

## The Rust skills in this repository

`.agents/skills/` holds eighteen Rust skills vendored from `po4yka/rust-skills`, and
`.claude/skills/` symlinks to them. Unlike the steps above this needs nothing from your machine: the
files are in the tree, so a fresh clone already has them.

Update them with the catalogue and never by hand:

```bash
npx skills update
```

That rewrites `.agents/skills/` and `skills-lock.json` from the catalogue. Run it in one repository,
read the diff, then apply the same change to every Ratatoskr repository whose stack is Rust.
`ratatoskr-workspace/.github/workflows/drift.yml` fails when one copy differs from the others.
