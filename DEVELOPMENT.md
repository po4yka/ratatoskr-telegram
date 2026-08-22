# Developing Ratatoskr Telegram

> Status: Implemented for plan item 1; items 2 through 10 are Proposed.  
> Last reviewed: 2026-08-22

## Current stage

Plan item 1 of `docs/IMPLEMENTATION_PLAN.md` is implemented and the commands marked **real** below
are real. The Cargo workspace, its pinned toolchain and its committed `Cargo.lock`; the
`ratatoskr-telegram-core`, `ratatoskr-telegram-telemetry`, `ratatoskr-telegram-http` and
`ratatoskr-telegram-persistence` library crates; the `ratatoskr-telegram-webhook` and
`ratatoskr-telegram-dispatcher` binaries; typed configuration loaded from `RATATOSKR__` environment
variables with unknown keys refused; the typed error hierarchy with its internal arm; the `tracing`
subscriber with optional OTLP span export and real trace ids without a collector; liveness,
readiness, Prometheus metrics and version on a per-role operator listener (webhook 9467, dispatcher
9468); SIGTERM draining; the first-version `telegram` schema in `schema.sql`, applied at startup to
a configured database; and the CI gate in `.github/workflows/ci.yml`.

Not present yet, in plan order: the Bot API client and secure webhook (item 2), identity/chat
binding and access control (item 3), the dispatcher's projections and outbound queue (item 4), and
everything after. No code path contacts Telegram. `teloxide` is declared as the Bot API dependency
in the workspace manifest for item 2 to take up.

The database is OPTIONAL for both roles at this milestone: nothing reads persisted data yet, so a
process configured without one starts, serves its probes, and reports no database check. A process
configured WITH an unreachable one starts too, and reports that check failing — when the first
feature writes through the pool, this becomes a refusal to start, as the sibling services treat a
dependency their routes cannot serve without.

## Toolchain

Rust/Tokio/Axum/SQLx, pinned by `rust-toolchain.toml`. The Telegram Bot API client is `teloxide`
(pinned in the workspace manifest, rustls TLS). PostgreSQL 17 locally through `compose.yaml`. There
are no migrations by development-status rule: one schema file, applied fresh, edited in place.

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

# webhook role, operator plane on 9467
RATATOSKR__TELEMETRY__LOG_FORMAT=pretty cargo run -p ratatoskr-telegram-webhook
# dispatcher role, operator plane on 9468
RATATOSKR__TELEMETRY__LOG_FORMAT=pretty cargo run -p ratatoskr-telegram-dispatcher

curl -s http://127.0.0.1:9467/health/live
curl -s http://127.0.0.1:9467/health/ready
curl -s http://127.0.0.1:9467/metrics | head
curl -s http://127.0.0.1:9467/version

# validate a configuration without starting anything (exit 0 or 78)
cargo run -p ratatoskr-telegram-webhook -- check-config

# stop cleanly; readiness flips to 503 during the drain while the listener still answers
kill -TERM <pid>
```

Both binaries start with no configuration at all: loopback admin listeners on their own default
ports, JSON logs, no exporter. A database appears only when `RATATOSKR__DATABASE__URL` is set, and
then the embedded `schema.sql` is applied at startup before readiness can pass.

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

CI uses synthetic updates only and never a production bot token; there is no bot token in this tree
at all until item 2 introduces the credential plumbing that keeps it in the secret store.

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
