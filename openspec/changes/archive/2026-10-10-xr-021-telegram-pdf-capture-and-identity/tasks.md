Environment for every command: `export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`, `TELEGRAM_TEST_DATABASE_URL` on the private PostgreSQL and `TELEGRAM_TEST_NATS_URL` on the private NATS (XR-021 local ports 57240 and 57241).

## 1. Deploy values target the allocated Edge (CONTRACTS S11)

- [x] 1.1 RED: add `services/webhook/tests/deployment_profile.rs::shipped_examples_target_the_allocated_edge` (both examples contain exactly `RATATOSKR__PLATFORM__AUDIENCE=edge` and `RATATOSKR__PLATFORM__BASE_URL=http://127.0.0.1:8080` and not `ratatoskr-platform`) and `crates/core/tests/platform_config.rs::defaults_target_the_allocated_edge_listener`; run both and confirm the assertions fail on the old values.
- [x] 1.2 GREEN: edit the two examples, `default_platform_base_url()` and the audience default in `crates/core/src/config/model.rs` (an empty audience stays refused), `DEVELOPMENT.md` and `deploy/README.md`; rerun both tests and the existing config tests.

## 2. Stale `.env.example` bind (CONTRACTS S05)

- [x] 2.1 RED: add `services/webhook/tests/deployment_profile.rs::env_example_documents_the_compiled_listener_defaults` (documents `RATATOSKR__WEBHOOK__BIND` with `127.0.0.1:8182`, never `9469`); run it and confirm it fails on the stale `9469`.
- [x] 2.2 GREEN: edit the `.env.example` comment and example value; rerun the test. No code change.

## 3. Blob captures use the blob route (CONTRACTS S11)

- [x] 3.1 RED: add `crates/platform-api/tests/client.rs::blob_capture_posts_the_blobref_to_the_blob_route` (the fake Platform accepts only `POST /v1/captures/blobs` with the exact `blob` body and answers 404 on `POST /v1/captures`) and keep `url_capture_still_posts_to_captures`; run and confirm the blob test fails with a 404-class refusal.
- [x] 3.2 GREEN: `Client::submit_capture` chooses the path by `CaptureSource` variant and keeps the body shapes; fix the doc comment that claimed Platform ignores blob members; rerun both tests.

## 4. PDF-only attachments and a reply for every capture failure (CONTRACTS S11)

- [x] 4.1 RED: in `services/webhook/tests/capture/` (child modules of `capture.rs`, which is at the line cap) add `a_photo_gets_an_unsupported_reply_and_no_platform_call`, `a_non_pdf_document_gets_an_unsupported_reply`, `a_platform_permanent_refusal_enqueues_a_could_not_save_reply`, `a_platform_outage_enqueues_a_temporarily_unavailable_reply` and `an_attachment_download_failure_enqueues_a_could_not_read_reply`; extend the fake Platform with `POST /v1/captures/blobs`, update the PDF happy path to assert the blob route, and align the existing outage, refusal, overrun and unsupported-media assertions with the new replies; run and confirm each fails because `telegram.outbound_jobs` is empty or the body is the old text, and the happy path fails on the route.
- [x] 4.2 GREEN: `select_attachment` accepts only `application/pdf` documents; extend the reply kinds with `CaptureUnavailable`, `CaptureRefused` and `AttachmentUnreadable` and the static S11 bodies; `handle_attachment` enqueues `AttachmentUnreadable` on file-resolution, download, media-type and store failure; `capture_run.rs` enqueues the matching reply on `TransientExhausted` and `PermanentRefusal` before settling `failed`; rerun the tests.
- [x] 4.3 REFACTOR (adds no test, changes no behaviour): move the attachment half of `worker.rs` into `worker/attachment.rs` to stay under the 850-line cap; rerun the capture tests.
- [x] 4.4 Update `README.md` and `docs/INTERFACES.md` (PDF documents only, images refused) and `DEVELOPMENT.md` plan item 6 wording; documentation cannot start from a failing test. Record the image-capture follow-up with the `refusal-only` label (see the handoff report).

## 5. Bind `identities.internal_user_id` on the first session exchange (CONTRACTS S11)

- [x] 5.1 RED: add a signature-only `Database::bind_internal_user` returning a neutral value and `crates/persistence/tests/bindings.rs::bind_internal_user_sets_the_column_and_is_idempotent` and `bind_internal_user_overwrites_a_changed_platform_mapping`; run and confirm the column stays NULL.
- [x] 5.2 GREEN: implement the conditional update and report whether a row changed; rerun.
- [x] 5.3 RED: add a signature-only `SessionSource::with_on_mint` and `crates/platform-api/tests/session.rs::the_on_mint_hook_runs_once_per_network_exchange_and_not_on_cache_hits` and `a_hook_failure_does_not_fail_the_exchange`; run and confirm the hook never runs.
- [x] 5.4 GREEN: store the hook and await it after the cache insert of every successful network exchange; rerun.
- [x] 5.5 RED: add `services/webhook/tests/capture/` test `a_first_capture_binds_the_senders_internal_user_id` and `services/dispatcher/tests/identity_binding.rs::a_notification_reaches_a_user_bound_by_a_session_exchange` (its own test binary, because it owns the process-global metrics recorder; it also asserts the `bound`, `unchanged` and `failed` outcomes of `telegram_identity_binding_total`); run and confirm the identity row keeps a NULL `internal_user_id` and the notification ends `NoEligibleChat`.
- [x] 5.6 GREEN: add `telegram_identity_binding_total`, the shared hook constructor `telegram_http::identity_binding_hook` (both roles run the same code), and install it in `services/webhook/src/intake/build.rs::build_capture_context` and `services/dispatcher/src/build.rs`; update the `schema.sql` comments and the docs that say the column stays NULL; rerun.

## 6. Durable, extractor-readable attachment root (CONTRACTS S11)

- [x] 6.1 RED: add `services/webhook/tests/deployment_profile.rs::attachment_root_is_on_the_durable_device_and_group_readable`; run and confirm it fails on the default boot-device root.
- [x] 6.2 GREEN: edit `webhook.conf.example`, the webhook unit (keeping the `/var/lib/ratatoskr-telegram` prefix of `ReadWritePaths`) and `deploy/README.md` (group, setgid `2750` install step, "Attachment blob root" section); rerun the deployment profile tests.

## 7. Concurrent binding replays (a defect the gate exposed)

`capture_recovery::concurrent_accepted_capture_recovery_enqueues_one_acknowledgement` failed in about four runs out of ten on the unmodified baseline: two updates from one sender race through the access gate and the loser of `bind_private_chat` hit the chat's unique key instead of seeing the winner's row. It is not part of the contracts but it blocks the repository gate, so the fix lives in this change.

- [x] 7.1 RED: add `crates/persistence/tests/bindings.rs::concurrent_replays_of_one_private_chat_binding_all_succeed` (two joined binds of one pair over a hundred fresh chats) and `a_chat_bound_to_another_actor_is_still_refused`; run and confirm the first fails with `private_chat_bindings_chat_id_key` and the second already passes.
- [x] 7.2 GREEN: `bind_private_chat` inserts with `on conflict do nothing` and then checks the chat's owner, returning the new `PersistenceError::ChatBoundToAnotherActor` for a different actor; rerun both and the capture recovery test repeatedly.

## 8. Final gate

- [x] 8.1 Run the fenced gate of `DEVELOPMENT.md` in order (fmt, clippy, 850-line check, debug build, tests, release build), `cargo deny --locked check`, the operator runbook checks, `openspec validate --all --strict` and `openspec validate --archived`, and observe every check pass.
