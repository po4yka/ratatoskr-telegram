# Design: service scaffold

## Context

The repository has documents and a fleet gate but no code. The sibling services — above all
`ratatoskr-platform` — already run this layer, and this change copies their structure rather than
inventing one: workspace layout (`crates/` + `services/`), `clippy.toml` size limits beside the
first manifest (a `fleet.yml` assertion), the `RATATOSKR__` figment loader with
`deny_unknown_fields`, the tracing/opentelemetry/metrics telemetry stack, the axum operator plane,
the sqlx pool with an embedded schema, and the gate workflow whose command list is mirrored in
`DEVELOPMENT.md`.

Development status is binding: first version only, no migrations — `schema.sql` is edited in place
and applied to fresh databases; the product is Ratatoskr.

## Goals / Non-Goals

Goals:

- A workspace that builds, passes the full gate, and runs both binaries locally against their
  operator plane.
- One mechanism per concern, copied from platform so an engineer can move between repositories.
- Tests written first for every behavior in the four delta specs.

Non-Goals:

- No Bot API client, webhook route, update parsing, or any Telegram network call (plan items 2+).
  `teloxide` is declared as the workspace dependency now so the choice is recorded and pinned; no
  crate depends on it yet.
- No NATS/bus, no outbox/inbox, no identity binding, no public listener. `RuntimeState` keeps its
  bus field out until item 4 needs it.
- No Dockerfile or release artifact job; CI builds and tests on the runner target.

## Decisions

### Two deployables now, sharing one harness

`services/webhook` and `services/dispatcher` exist from the start because the repository's planned
shape names them and plan items 2–4 fill them in; each binary is a role constant plus the shared
`run(role, routes)` call. Roles are compiled in (`webhook` | `dispatcher`), never configured.
Default admin ports `9467`/`9468` continue platform's allocation block (9464–9466) and stay clear of
the ports `DEPLOYMENT_TARGET.md` records as held.

Alternative considered: one binary with a mode flag. Rejected — a configurable role lets an operator
start two processes that lie in every metric label they emit.

### Configuration: figment, env-only, value-free error reports

Same shape as `platform-core`: built-in per-role defaults merged with `RATATOSKR__ENV` variables,
`deny_unknown_fields` on every table so a typo fails loudly, semantic validation that collects all
violations, exit code 78, and a `check-config` subcommand. Error reports name keys, never values, so
a secret cannot leak through a startup failure. There is deliberately no configuration file.

Fields at this milestone: `admin.bind`, optional `database` (URL as `SecretString`,
max connections, acquire timeout), `shutdown.drain_seconds`/`grace_seconds`, `telemetry`
(log format, filter, optional OTLP). Bot token, webhook secret and public-listener fields arrive
with the webhook item that consumes them — config fields without a consumer are untestable surface.

Validation rules carried over where they apply here: log filter must parse as an EnvFilter
directive; OTLP endpoint scheme must be `https` unless the host is loopback; database URL scheme
must be `postgres`/`postgresql`; shutdown windows bounded; OTLP timeout bounded. Rules are numbered
V1… in code comments so future rules append without renumbering.

### Telemetry: registry + optional OTLP layer + format layer, stdout JSON by default

Copied from `ratatoskr-platform-telemetry`: `EnvFilter` → `tracing_opentelemetry` layer → JSON or
pretty fmt layer writing to stdout; W3C trace-context propagator; Prometheus recorder installed
globally with shared latency buckets; build info gauge. An exporterless tracer provider still mints
real sampled trace ids, which is what puts a working `trace_id` in every line before any collector
exists. OTLP over tonic with rustls; header secrets are `SecretString`s read exactly once to build
sensitive-marked metadata. Metric names live in one module; only `telegram_build_info` and
`telegram_readiness` exist at this milestone.

### Errors: typed per crate, two-arm boundary error reserved

`ConfigError`, `TelemetryError`, `PersistenceError` are concrete enums. `TelegramError` follows the
error-contracts two-arm pattern but ships with only the internal arm (`Subsystem`, boxed source)
because the client-visible arm gets its variants from routes that reject callers — inventing
failure kinds with no producer is how dead taxonomy grows. The rejected arm arrives with the
webhook's public surface (item 2), which is also when contract `ErrorCode`/`SafeMessage` types enter
via the pinned `ratatoskr-contracts` git dependency. Workspace lints deny `unsafe_code`, panics,
`unwrap`/`expect` outside tests, and require docs on public items.

### Persistence: pool + embedded schema + advisory lock

One `Database` handle wrapping a bounded sqlx pool. `schema.sql` is `include_str!`d into the binary
so a deployment cannot drift from its schema. Application takes a transaction-scoped advisory lock,
checks for the schema's presence, applies the file if absent, commits — idempotent under concurrent
starts and all-or-nothing under PostgreSQL's transactional DDL. A background prober pings every
5 s and publishes the result to readiness; probes never run inside readiness requests. The lock key
is distinct from platform's. Integration tests create disposable databases named after the test
through a `test-support` feature, using `TELEGRAM_TEST_DATABASE_URL`.

### Operator plane and shutdown: platform harness minus the public router

`admin_router` serves the four endpoints with `no-store`; readiness checks render sorted, map-free
JSON. `run(role, routes)` sequences config → telemetry → startup span → listeners →
startup-complete → signal → drain-then-close → flush → exit 0/1/78. The public-router middleware,
body limits and error envelope are deferred to the webhook item; `PublicRoutes` exists so binaries
already express "no public listener".

### Schema content: empty on purpose

`schema.sql` creates the `telegram` schema with ownership documentation and no tables. Tables are
specified by `docs/DATA_MODEL.md` and each arrives with the feature that owns it (update dedupe in
item 2 writes `telegram_updates`). Placeholder tables nobody reads would be drift the day they land.

### Gate: platform's ci.yml minus jobs this repo cannot fill yet

Gate job with PostgreSQL service container, then: fetch, deny, fmt, clippy `-D warnings`, the
file-length awk (850), debug build, test, release build, and the DEVELOPMENT.md sync check.
No artifact job (no Dockerfile), no NATS container (nothing consumes it). `TELEGRAM_TEST_DATABASE_URL`
matches `compose.yaml`.

## Risks / Trade-offs

- [Two near-empty binaries look like ceremony] → They are the roles items 2–4 implement; deleting a
  binary later would be cheaper than splitting one later.
- [Empty schema.sql could be mistaken for done persistence] → The spec scopes it explicitly;
  README says tables arrive with their features.
- [Teloxide declared but unused] → It pins intent for item 2; cargo deny does not audit it until a
  crate depends on it, which the Bot API task must do.
- [`missing_docs` deny raises the bar for every public item] → That is the house style inherited
  from platform; cheaper to adopt before code exists than retrofit.

## Migration Plan

Not applicable — there is no deployed instance and no data. The change lands on a task branch,
merges to `main`, and pushes once the gate is green.

## Open Questions

None. Port allocations 9467/9468 should be recorded in the shared deployment-target document when
this service deploys; that edit belongs to the workspace store, not this repository.
