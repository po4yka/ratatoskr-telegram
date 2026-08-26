# Design — URL/article capture command and message flow

## Context

Items 1-4 left the service with admission, access control, durable delivery, and projections, but no product: the worker's post-authorization arm settles everything `Processed` without acting (`services/webhook/src/intake/worker.rs`, `process_one`), and nothing publishes into the dispatcher's projection feed (`DispatcherRuntime::projection_feed()` holds an unfed mpsc seam awaiting its transport). The verified Platform surface offers: `POST /v1/captures` (Bearer session + mandatory `Idempotency-Key`; replay returns the original operation; same-key-different-body is 409), `GET /v1/operations/{id}`, `GET /v1/operations/{id}/events` (SSE frames `event: progress`, `id: <progress uuid>`, data `{status, stage, percentage, observed_at}`, `Last-Event-ID` resume, server closes at terminal), and `POST /v1/sessions/telegram` which exchanges a single-use Ed25519-signed assertion - issuer fixed to `ratatoskr-telegram`, audience-checked, nonce-redeemed - for a one-hour bearer session. ratatoskr-extractor consumes the resulting `content.capture.requested.v1` commands end to end and reports Queued/Succeeded/Failed with no intermediate stages.

Constraints that bind this design: development status (schema edited in place, one version); clippy.toml size limits (fn ≤ 100 lines, args ≤ 7, file ≤ 850); tests-first task pairs; hand-written fakes against local harnesses only; no raw URLs or user identifiers in logs or metric labels; the renderer's honesty rules (no fabricated stages or summaries).

## Goals / Non-Goals

**Goals:**

- One authorized URL becomes exactly one Platform operation tracked in exactly one Telegram message, honestly rendered through accepted, running, and terminal states.
- Double-sends converge on one operation through deterministic idempotency keys; deliberate retries after failure deliberately do not.
- The projection feed gains its first real producer without changing consumer internals - the NATS adapter still lands at workspace integration.
- Secrets stay configuration-only; the signing key never leaves this service, and no test contacts Telegram or a live Platform.

**Non-Goals:**

- A cancel command (Platform serves no cancellation route today; a workspace changeset must add the capability first).
- Rich result rendering (title/TLDR/key points) - the snapshot carries references and safe lines only, and no route returns document content; waits for the analysis pipeline plus a read surface.
- Retry buttons (callback tokens are item 8), file/forward inputs (item 6), Mini App initData validation (item 9), NATS transport, notification preferences.

## Decisions

### D1 — One new crate speaks Platform

`crates/platform-api` mirrors `crates/bot-api`'s shape: `Client::new(credential-free)` building a `reqwest` client with whole-call timeout and rustls, harness-pointable base URL, closed `thiserror` taxonomy mapping Platform's envelope classes (`network`, `timeout`, `unauthenticated`, `client_error { status }`, `conflict`, `rate_limited`). Methods: `submit_capture(&self, session, idempotency_key, url)`, `read_operation`, `stream_events` (SSE), `exchange_assertion`. Only this crate imports Platform wire shapes; everything downstream sees typed values. The assertion issuer also lives here: claims `{issuer:"ratatoskr-telegram", subject, audience, nonce, issued_at, expires_at}` serialized, Ed25519-signed, encoded `base64url(payload).base64url(signature)` - byte-formats proven by round-trip tests against the verifier's documented shape.

### D2 — Sessions are cached per sender, minted by signed assertion

The worker obtains a bearer credential per capture through the exchange route. A small in-memory cache keyed by Telegram user id stores `(credential, expires_at)` with a safety margin (refresh 5 minutes early) and single-flight refresh so concurrent captures share one exchange. Subject is the Telegram user id string; Platform resolves or creates the internal user keyed on `(provider=Telegram, external_id)`, so no cross-service identifier coupling enters this schema and `telegram.identities.internal_user_id` stays untouched until the item-9 binding flow owns it.

### D3 — Intent parsing is a pure function with a pinned grammar

`services/webhook/src/intake/intent.rs`: `parse(text) -> Option<CaptureIntent>` where `CaptureIntent { kind: Summarize, url: NormalizedUrl }`. Grammar: `/summarize <url>` with exactly one argument, or the entire trimmed text being one http(s) URL; scheme/host lower-cased; length capped at 2048 (Platform's limit); anything else parses to `None` and the update settles `unsupported`. The idempotency key is `hex(SHA-256("capture.v1|{telegram_user_id}|{normalized_url}"))`; the retry-after-failure flow derives `hex(SHA-256("capture.v1|{user}|{url}|retry:{failed_operation_id}))`. Both derivations are pure and unit-tested for stability.

### D4 — The domain action replaces one arm, preserving settlement semantics

In `process_one`, `Ok(None) => UpdateState::Processed` becomes the capture arm: resolve intent → session → `submit_capture` (at most two attempts for transient classes) → on success write, in order, `ensure_operation_binding`, the intent row, and the acknowledgment send job, then settle `Processed`; on exhausted/permanent failure settle `Failed` with a class metric and send nothing. Settlement remains exactly-one-terminal; the update dedupe layer above is untouched, so a redelivered update after uncertain failure replays safely - the second submission hits Platform's idempotency ledger and converges on the original operation.

### D5 — Pre-created bindings close the unsent-ack race

The binding row exists before any frame can arrive, so early progress is applied rather than counted `Unbound`. If a render lands while `message_id` is still null (send not yet acknowledged), the existing permanent-edit-failure path does the right thing already: the edit fails permanently, unbinds, and the next revision sends fresh - machinery item 4 built and tested. No new guard is invented here.

### D6 — The follower is a bindings-table-driven loop feeding the existing seam

`services/dispatcher/src/follow.rs`: a scan loop polls non-terminal bindings every `PLATFORM__FOLLOW_SCAN_INTERVAL_SECS`, diffs them against an in-flight set (bounded cap), and spawns one task per operation consuming `stream_events`. Each SSE frame maps onto the existing `OperationEvent` - event id from the frame's progress uuid (inbox dedupe applies unchanged), `occurred_at` from `observed_at`, status through the closed vocabulary, stage/percentage passed through - and pushes into `projection_feed()`. Transport errors reconnect with capped jittered backoff carrying `Last-Event-ID`; a terminal frame or server close ends the task. After restart the scan finds the same non-terminal bindings, so recovery needs no journal. The NATS adapter later replaces only this loop's ingress; consumer internals stay frozen.

### D7 — Outbound payloads become structured; markup rides end to end

`telegram.outbound_jobs.body text` becomes `payload jsonb` holding `{text, parse_mode?, reply_markup?}` (development status: in-place edit, no migration); `content_hash` hashes the canonical serialization, so identical re-renders stay suppressed and markup-only changes genuinely edit. The renderer keeps returning a String for progress bodies; a terminal composer wraps it into a payload adding the fallback hyperlink and deep-link button. `BotApiSink`'s two methods take the structured payload; the bot-api client's `send_message`/`edit_message_text` gain optional parse-mode/markup parameters mapped to teloxide types, omitting both fields when absent.

### D8 — Deep-link intents are the first writer of `telegram.interaction_intents`

Columns: app-minted UUIDv7 PK (the opaque token itself), `bot_id`, `telegram_user_id`, `chat_id`, kind CHECK `'operation_status'`, `operation_id`, `source_url`, `created_at`, `expires_at` (30-day constant TTL, documented). Repositories: `insert_intent`, `find_live_intent(id, telegram_user_id)` filtering expiry and owner. The webhook inserts one row per accepted capture; the terminal composer resolves it back by `(intent, owner)` when building the button target `https://t.me/{bot_username}?startapp={intent_id}`. Bot username comes from a startup `get_me` in the dispatcher (the webhook already performs one), kept in runtime state - never fetched per render. Callback-token machinery (one-time consumption, confirmation context) remains item 8; these intents are pure presentation links.

### D9 — Configuration continues the numbering; both roles demand Platform

`PlatformConfig` joins `TelegramConfig` with `#[serde(deny_unknown_fields)]`: `BASE_URL` (default `http://127.0.0.1:9463` for the development harness; https-required-off-loopback mirroring the Bot API rule), `TIMEOUT_SECONDS` (1..=60, default 10), `AUDIENCE` (1..=128 chars), `ASSERTION_SIGNING_KEY` (secret; must decode to a 32-byte Ed25519 key), plus `FOLLOW_SCAN_INTERVAL_SECS` under it for the follower cadence. Validation adds V16 (value rules) and V17 (role requirements): both roles refuse to start without the section, reported beside every other missing requirement, secret never echoed. Boot expectations in both services' tests update accordingly.

### D10 — Telemetry counts classes, never content

New counters: `telegram_capture_intents_total{outcome}` (parsed|unsupported), `telegram_capture_submissions_total{class}` (accepted|replayed|transient-exhausted|permanent), `telegram_operation_follows_total{event}` (started|resumed|ended|dropped), `telegram_platform_sessions_total{event}` (exchanged|reused). Labels stay closed vocabularies; URLs, usernames, and ids appear nowhere.

## Risks / Trade-offs

- [Assertion exchange on the submission path adds latency] → one round trip on cache miss only; the cache absorbs bursts; the update pipeline is asynchronous so webhook acknowledgment latency is unaffected.
- [SSE fan-out grows with live operations] → bounded follower cap and per-operation streams that end at terminal; a single-owner deployment holds tens, not thousands; queue-depth and follow metrics expose pressure.
- [Terminal renders before the ack send completes] → the existing unbind-and-resend fallback converts the doomed edit into a fresh send; worst case is one extra message, never silence.
- [`payload jsonb` changes job storage] → development status makes this an in-place edit with zero migrated rows; the hash discipline keeps dedupe semantics identical.
- [Platform outage stalls capture processing] → bounded retries then honest `failed` settlement; updates are not silently dropped and the class counter distinguishes exhaustion from refusal.
- [Wall-clock latency assertions flake under loaded machines] → new tests follow the house pattern: injected clocks and harness servers, no sleeps, timing assertions only where the contract is timing.

## Migration Plan

No database survives a schema change: fresh databases get `payload jsonb` and `telegram.interaction_intents` from the edited `schema.sql`; nothing migrates. Deployment note for the runbook (updated in this change): operators generate one Ed25519 keypair, configure the public half as Platform's assertion verification key and the private half as `RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY`; both binaries now refuse to start without the Platform section, which is the documented trajectory rather than a regression.

## Open Questions

None blocking this slice. The cancel capability and rich-result read surface are named deferrals requiring workspace changesets; callback tokens and dialogue state arrive with item 8.
