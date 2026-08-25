## 1. Platform API client crate

- [ ] 1.1 Scaffold `crates/platform-api` with `Client::new`, the closed error taxonomy (`network`, `timeout`, `unauthenticated`, `client_error{status}`, `conflict`, `rate_limited`), and the typed wire values (`OperationAccepted`, `OperationSnapshotView`, `ProgressFrame`). Cannot start from a failing test: this task creates types and dependency wiring only, no behavior to assert.
- [ ] 1.2 RED: add `crates/platform-api/tests/client.rs` with the axum harness pattern from `crates/bot-api/tests/client.rs` and test `submit_capture_posts_key_bearer_and_url_and_parses_operation`: calling `submit_capture` with session `sess-1`, key `key-1`, url `https://example.test/a` records `POST /v1/captures` carrying header `idempotency-key: key-1`, `authorization: Bearer sess-1`, body `{"url":"https://example.test/a"}`, and resolves to operation id parsed from the served 202 body; confirm it fails because the method reports unimplemented, not because of typing
- [ ] 1.3 GREEN: implement the capture submission request/response path; rerun until 1.2 passes
- [ ] 1.4 RED: extend `crates/platform-api/tests/client.rs` with `platform_error_classes_map_from_status_envelopes` pinning the taxonomy: 401/403 → `unauthenticated`; 409 → `conflict`; 429 → `rate_limited`; other 4xx → `client_error`; 5xx and closed port → transient classes; confirm it fails on the unimplemented classifier
- [ ] 1.5 GREEN: implement status/envelope classification; rerun until 1.4 passes
- [ ] 1.6 RED: add `read_operation_parses_snapshot_fields` asserting the snapshot view surfaces status, stage, errors' safe lines, warnings, and result references from a recorded `GET /v1/operations/{id}` body while ignoring unknown additive fields; confirm it fails on the unimplemented reader
- [ ] 1.7 GREEN: implement the operations read; rerun until 1.6 passes
- [ ] 1.8 RED: add `stream_events_delivers_frames_resumes_and_stops_at_terminal` against a harness serving `text/event-stream`: frames arrive typed with their progress uuid and observed timestamp, a reconnect carries `Last-Event-ID`, and the stream ends after the terminal frame without extra reads; confirm it fails on the unimplemented stream
- [ ] 1.9 GREEN: implement the SSE consumer over reqwest's byte stream; rerun until 1.8 passes

## 2. Assertion issuance and session cache

- [ ] 2.1 RED: add `crates/platform-api/tests/assertion.rs::assertion_matches_the_verifier_shape`: signing claims for subject `900700601`, audience `ratatoskr-edge`, nonce and expiry produce the compact `base64url(payload).base64url(signature)` form whose payload JSON carries exactly the six documented member names and whose signature verifies under the paired public key using the same Ed25519 construction Platform documents; confirm it fails because issuance does not exist
- [ ] 2.2 GREEN: implement Ed25519 issuance over the configured secret key; rerun until 2.1 passes
- [ ] 2.3 RED: add `crates/platform-api/tests/session.rs::sessions_are_exchanged_once_and_refreshed_before_expiry` with a FakeClock: two captures within lifetime hit `/v1/sessions/telegram` once; advancing past the refresh margin exchanges again; concurrent callers share one exchange (single-flight); confirm it fails on the missing cache
- [ ] 2.4 GREEN: implement the per-sender cache with margin and single-flight; rerun until 2.3 passes

## 3. Intent parsing and deterministic keys

- [ ] 3.1 RED: add `services/webhook/src/intake/intent.rs` unit tests: `bare_https_url_and_summarize_form_parse_to_capture_intents`, `host_and_scheme_case_normalize_to_one_canonical_url`, `non_http_schemes_free_text_and_missing_argument_do_not_parse`, and `urls_over_the_platform_limit_do_not_parse`; confirm each fails for the stated absence, not a typo
- [ ] 3.2 GREEN: implement the pure parser and normalization; rerun until 3.1 passes
- [ ] 3.3 RED: add intent-key tests `keys_are_stable_across_repeats_and_normalizations` (same sender + URL twice → identical hex key; host-case variant → identical) and `retry_keys_salt_with_the_failed_operation` (retry derivation differs from the base key and encodes the failed operation); confirm they fail on missing derivations
- [ ] 3.4 GREEN: implement both derivations as pure functions; rerun until 3.3 passes

## 4. Schema and persistence

- [ ] 4.1 RED: add `crates/persistence/tests/intents.rs::interaction_intents_exist_with_expected_shape` asserting the table's columns, CHECK vocabulary (`operation_status`), app-minted UUID PK without default, and timestamptz expiry; confirm it fails on the missing relation before touching `schema.sql`
- [ ] 4.2 GREEN: declare `telegram.interaction_intents` in root `schema.sql`; switch `telegram.outbound_jobs.body` to `payload jsonb`; rerun until 4.1 passes
- [ ] 4.3 RED: add repository tests in `crates/persistence/tests/intents.rs`: `inserted_intent_is_found_by_owner_until_expiry` (live lookup succeeds for owner, returns nothing past expiry or for another user) and `outbound_payload_round_trips_with_markup` (enqueue a job whose payload carries text plus keyboard; claim returns it bit-identical); confirm both fail on missing repositories
- [ ] 4.4 GREEN: implement `crates/persistence/src/intents.rs` and the outbound payload column read/write through the existing enqueue/claim functions; rerun until 4.3 passes

## 5. Bot API markup and structured payloads

- [ ] 5.1 RED: extend `crates/bot-api/tests/client.rs` with `send_and_edit_carry_parse_mode_and_reply_markup` (harness asserts `parse_mode` HTML and the exact keyboard JSON when given, and neither field when absent); confirm it fails against current signatures
- [ ] 5.2 GREEN: widen `send_message`/`edit_message_text` with optional parse mode and inline keyboard; update existing call sites; rerun until 5.1 passes
- [ ] 5.3 RED: extend `services/dispatcher/src/outbound/sender/sink.rs` tests (in `tests/delivery.rs`) with `structured_payloads_reach_the_sink_verbatim` asserting the recording fake sees text, parse mode, and buttons unchanged through claim→deliver, and `payload_hash_distinguishes_markup_only_changes` (same text, added keyboard → real edit, not suppression); confirm both fail before the sink changes
- [ ] 5.4 GREEN: thread structured payloads through job claim, hash, supersede, and sink; rerun until 5.3 passes

## 6. Webhook worker domain action

- [ ] 6.1 RED: add `services/webhook/tests/capture.rs` with a Fixture combining the intake fixture, a fake Bot API server, and a fake Platform harness (exchange + captures routes): `authorized_url_message_submits_capture_and_enqueues_ack` seeds an enabled owner, delivers a bare-URL message, asserts the exchange was called once, `POST /v1/captures` carried the derived key and bearer, the update settled processed, one binding row pre-exists for the returned operation, and exactly one send job referencing it was enqueued; confirm it fails before the worker arm exists
- [ ] 6.2 GREEN: implement the capture arm (intent → session → submit → binding + intent row + ack job → settle processed) behind the platform client seam; rerun until 6.1 passes
- [ ] 6.3 RED: extend `services/webhook/tests/capture.rs`: `resending_the_same_url_reuses_the_operation_without_a_second_ack` (second delivery replays the original operation id; still exactly one live binding and one send job for it), `unsupported_text_settles_unsupported_without_platform_calls` (`/summarize` bare, free text), and `summarize_command_parses_like_a_bare_url`; confirm they fail
- [ ] 6.4 GREEN: close the gaps those tests expose; rerun until 6.3 passes
- [ ] 6.5 RED: add `platform_outage_fails_boundedly_without_ack` (fake Platform refuses connections; assert exactly the bounded attempt count, update settles failed, zero outbound jobs, class metric incremented) and `permanent_refusal_settles_immediately` (401 answer → single attempt, failed settlement); confirm they fail
- [ ] 6.6 GREEN: implement retry classification and settlement; rerun until 6.5 passes

## 7. Dispatcher follower

- [ ] 7.1 RED: add `services/dispatcher/tests/follow.rs` with a fake Platform SSE harness: `non_terminal_bindings_are_followed_once_each_after_restart` seeds four bindings (three non-terminal, one terminal), runs scan + follower tasks against the harness, restarts the runtime, and asserts streams opened exactly for the three; confirm it fails before the follower exists
- [ ] 7.2 GREEN: implement the scan/diff/follow loop feeding `projection_feed()` with mapped frames; rerun until 7.1 passes
- [ ] 7.3 RED: extend `services/dispatcher/tests/follow.rs`: `frames_map_onto_projection_events_with_dedupe` (accepted + running + duplicate running frames yield the guard outcomes the consumer already defines - two accepted events, one duplicate count), `terminal_frame_ends_the_follow` (stream closes after succeeded; no further reads), and `transport_drop_resumes_with_last_event_id` (harness observes the resume header and replays one seen frame which deduplicates away); confirm they fail
- [ ] 7.4 GREEN: implement frame mapping, terminal stop, and resume/backoff; rerun until 7.3 passes

## 8. Terminal composition

- [ ] 8.1 RED: add `services/dispatcher/src/projection/compose.rs` unit tests: `succeeded_terminal_composes_links_and_button` (body keeps the escaped status lead, adds the source hyperlink, and the reply_markup carries exactly one url button targeting `https://t.me/{username}?startapp={intent}`), `failed_terminal_composes_guidance_without_retry_button` (failed lead + escaped safe line + resend guidance, empty markup), `missing_intent_or_username_degrades_to_text_only`, and `non_terminal_events_never_compose_markup`; confirm they fail on the missing composer
- [ ] 8.2 GREEN: implement the composer over binding + intent lookup and startup bot username; rerun until 8.1 passes
- [ ] 8.3 RED: add end-to-end `services/dispatcher/tests/end_to_end.rs::capture_lifecycle_renders_progress_then_completion_with_links` driving fake Bot API + fake Platform: seed owner via webhook fixture flow, deliver a URL message, feed accepted→running→succeeded frames through the follower, assert the chat saw one ack send then throttled edits then exactly one terminal edit carrying parse mode, hyperlink, and button, with the intent resolvable only by its owner; confirm it fails before wiring closes
- [ ] 8.4 GREEN: close wiring gaps until 8.3 passes; keep both mains thin role constants over their harness calls

## 9. Configuration and boot expectations

- [ ] 9.1 RED: add parse/validation tests in `crates/core/tests/platform_config.rs`: `platform_section_parses_with_defaults_and_unknown_keys_refused`, `platform_value_rules_violations_name_keys_without_echoing_secrets` (bad scheme off loopback, malformed signing key, empty audience, out-of-range timeout), and `both_roles_require_the_platform_section` (each binary's validation names the missing Platform keys); confirm they fail
- [ ] 9.2 GREEN: add `PlatformConfig` with V16 value rules and V17 role requirements; rerun until 9.1 passes
- [ ] 9.3 RED: update `services/webhook/tests/boot.rs` and dispatcher boot expectations: unconfigured binaries exit 78 naming `PLATFORM__*`; configured-with-loopback-harness binaries reach ready; confirm the changed expectations fail first
- [ ] 9.4 GREEN: thread the requirements through startup factories (webhook builds the platform client; dispatcher performs its startup `get_me` and holds username state); rerun until 9.3 passes

## 10. Telemetry and change verification

- [ ] 10.1 RED: add `telemetry_counts_classes_without_content` (in `services/webhook/tests/capture.rs` and `services/dispatcher/tests/follow.rs`) asserting exposition contains `telegram_capture_submissions_total{class=...}`, `telegram_operation_follows_total{event=...}` after driving one exhausted submission and one follow lifecycle, with no URL, username, or identifier in any label or log line; confirm it fails
- [ ] 10.2 GREEN: add the metric constants and instrument the paths; rerun until 10.1 passes
- [ ] 10.3 Run the full DEVELOPMENT.md gate command list with `TELEGRAM_TEST_DATABASE_URL` pointing at the local instance plus `openspec validate --strict`; all come back clean
- [ ] 10.4 Inspect `git diff` for leaked secrets, tokens, real identifiers, or raw URLs in logs/metric labels; confirm fixtures stay synthetic
- [ ] 10.5 Update README status paragraph, DEVELOPMENT.md stage/configuration/local-run sections, and record item 5 implemented in `docs/IMPLEMENTATION_PLAN.md` order; archive the change only after 10.3 is green and every task above is ticked
