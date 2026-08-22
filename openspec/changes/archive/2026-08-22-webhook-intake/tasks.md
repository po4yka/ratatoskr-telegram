# Tasks: secure webhook intake

## 1. Workspace wiring

- [x] 1.1 Add `crates/bot-api` to the workspace members and move `teloxide` onto the audit surface: workspace dependency entries for `teloxide`, `reqwest`, `subtle`; promote `http-body-util` from dev-only to a shared dependency; give `services/webhook` a library target. No behavior exists yet; manifest and generated lockfile. Verify: `cargo metadata --format-version 1 > /dev/null` and `cargo fetch --locked` resolve.

## 2. Configuration

- [x] 2.1 Write `crates/core/tests/webhook_config.rs`: a loopback harness base URL validates and a plain-http public host is refused naming `bot_api.base_url`; timeout bounds enforced; secret shorter than 16 or outside `[A-Za-z0-9_-]` refused naming `webhook.secret_token` without echoing it; body-cap bounds enforced; equal admin/public binds refused; webhook role unconfigured names every missing requirement (`bot_api.token`, `webhook.secret_token`, `database.url`) while the dispatcher loads clean on defaults. Confirmed red against the stashed scaffold: the `webhook_config` target does not exist there.
- [x] 2.2 Implement `BotApiConfig`/`WebhookConfig` in the config model with defaults, validation rules V9–V13 appended to the value-free report, and role requirements, until the tests pass. (8 new tests green.)

## 3. Schema and persistence

- [x] 3.1 Write `crates/persistence/tests/updates.rs`: fresh database has `telegram.updates` with the composite key; first insert reports inserted, same pair again reports duplicate leaving one row; a different bot id is not a duplicate; settlement moves an admitted row to a terminal state with a timestamp; settling an unknown pair fails writing nothing. Requires PostgreSQL via `TELEGRAM_TEST_DATABASE_URL`. Confirmed red against the stashed scaffold: the `updates` target does not exist there.
- [x] 3.2 Edit `schema.sql` in place adding `telegram.updates` (no migration ledger) and implement the typed record/settle queries in `crates/persistence/src/updates.rs` until the tests pass. (5 tests green.)

## 4. Bot API client crate

- [x] 4.1 Write `crates/bot-api/tests/parse.rs` over recorded synthetic fixtures: message and callback_query updates parse with typed ids/kinds; an unknown kind key parses as unrecognized rather than failing; missing `update_id` and non-JSON bodies fail. Confirm red: the crate exports nothing.
- [x] 4.2 Write `crates/bot-api/tests/client.rs` against a local axum harness server: `get_me` resolves identity from a recorded response; API error body surfaces as the api class carrying the description and never the token; 429+retry_after surfaces as rate_limited with the delay; closed port surfaces as network; `send_message`, `edit_message_text`, `answer_callback_query`, `send_chat_action`, `set_webhook` post their typed payloads (path addresses the bot; secret only where that method's contract carries it). Confirm red: no client type.
- [x] 4.3 Implement `crates/bot-api/src/lib.rs`: `Client` wrapping teloxide's bot handle with configured base URL and reqwest timeout, the six methods, `BotApiError` taxonomy, update-type re-exports, fixtures embedded for reuse — until both suites pass. (15 tests green: 10 client, 5 parse.)

## 5. Lifecycle: optional public listener

- [x] 5.1 Rewrite `services/webhook/tests/boot.rs` expectations: the dispatcher boots on documented defaults and keeps warn-and-degrade on an unreachable database (readiness failing by name); the webhook now requires full intake configuration — boots ready against a real disposable database plus an in-test harness Bot API server, refuses exit 1 when that database is unreachable, and `check-config` exits 78 unconfigured (naming the missing keys) and 0 fully configured. Confirmed behavioral red against the stashed scaffold's binaries: they ignore the intake configuration and keep the old database tolerance.
- [x] 5.2 Extend `telegram_http::run(role, routes)` with the `PublicRoutes` factory seam, the second listener inside the existing drain sequence, and the webhook role's unreachable-database refusal, until boot passes. (6 boot tests green.)

## 6. Intake pipeline

- [x] 6.1 Write `services/webhook/tests/intake.rs` (in-process router via tower `oneshot`, disposable database, queue receiver held by the test): missing/wrong secret 401 with no write; oversized declared and streamed bodies 413 naming the limit with no write; wrong method 405; wrong content type 415; malformed JSON acked 200 with no write and no row; unsupported kind accepted 200, one row recorded as unsupported; valid update 200, one row accepted and exactly one queued; duplicate delivery dropped once-ever including out-of-order older ids while unseen older ids still process; requests complete promptly with the worker blocked then all queued items process after release; saturated queue 503 with no row; closed pool 503; worker settles processed/unsupported end-to-end. Confirmed red against the stashed scaffold: the `intake` target does not exist there, and its previous boot suite passes unmodified.
- [x] 6.2 Implement the intake pipeline in `services/webhook/src/` — constant-time secret gate, admission order (method, content type, declared size, capped streamed read), schema parse, reserve-then-insert-then-send handoff, bounded queue, classifying worker, metric constants in `crates/telemetry/src/metrics.rs` — and wire `main.rs` through `get_me` + the public router factory, until the suite and boot pass. (15 intake tests green, including the constant-time comparison unit test; 6 boot tests green.)

## 7. Documentation

- [x] 7.1 Document the new variables in `.env.example`, the intake flow and local-run requirements in `DEVELOPMENT.md`, and move the README status/layout/project-status sections to plan item 2. No failing test can drive prose; verified by review and the gate's sync check.

## 8. Gate and archive

- [x] 8.1 Full gate locally, every command in the DEVELOPMENT.md list in order, green.
- [x] 8.2 Tick all tasks, archive the change, `openspec validate --archived` green, merge to `main`, push, remove worktree and branch.
