Environment for every command: `export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`, `TELEGRAM_TEST_DATABASE_URL` on the private PostgreSQL and `TELEGRAM_TEST_NATS_URL` on the private NATS (XR-021 local ports 57840..57859). The authorized-broker test needs `nats-server` on `PATH`.

## 0. Plan

- [x] 0.1 Open this change (proposal, design, spec deltas, tasks), citing XR-021 CONTRACTS.md R4-08, and run `openspec validate --all --strict`. Documentation, so it cannot start from a failing test.

## 1. The key describes the body (CONTRACTS R4-08 rule 1)

- [x] 1.1 RED: add `crates/platform-api/tests/client.rs::request_parts_are_exactly_what_submit_capture_sends`, the webhook cases `a_pdf_sent_plain_and_then_forwarded_yields_two_accepted_operations`, `a_url_sent_bare_and_then_forwarded_yields_two_accepted_operations`, `the_same_message_sent_twice_converges_on_one_operation`, `a_different_media_type_for_the_same_bytes_is_a_separate_operation` against a fake Platform that replays only an identical key and body and answers 409 for the same key with another body, and the `intent.rs` unit test that different bodies never share a key; with a signature-only `request_parts`, run them and confirm they fail on their assertions (a 409 and a refusal reply today).
- [x] 1.2 GREEN: implement `request_parts`, have `submit_capture` send exactly that value, derive the `capture.v2` key in `intake/intent.rs` over it, delete `capture_key` and `blob_capture_key`, update every call site and make `retry_key` follow the derivation; rerun. Name the break (every key changes) in the commit body.

## 2. A conflict is transient (CONTRACTS R4-08 rule 2)

- [ ] 2.1 RED: add `a_conflict_is_retried_then_answered_unavailable_not_refused` and `a_conflict_that_clears_on_the_second_attempt_is_accepted` in `services/webhook/tests/capture_failures/mod.rs`; run them and confirm the conflict is classed a permanent refusal today.
- [ ] 2.2 GREEN: add `PlatformError::Conflict` to `is_transient`; rerun.

## 3. A rejected credential is dropped and the capture retried once (CONTRACTS R4-08 rule 3)

- [ ] 3.1 RED: add `a_rejected_session_is_exchanged_again_and_the_capture_succeeds` (URL and blob), `a_second_401_in_a_row_is_a_permanent_refusal` and `an_invalidation_never_removes_a_newer_credential`; run them and confirm the first fails because the cached credential is never dropped.
- [ ] 3.2 GREEN: invalidate, exchange and submit once more on `PlatformError::Unauthenticated` in `capture::submit`; update `permanent_refusal_settles_immediately` to the new attempt count; rerun.

## 4. The ack wait is stated (CONTRACTS R4-08 rule 4)

- [ ] 4.1 RED: add `crates/core/tests/notification_config.rs::an_ack_wait_other_than_thirty_is_refused` and the deployment-profile case for the dispatcher example comment and the guide; run them and confirm they fail on the accepted 29 and 31 and the missing comment.
- [ ] 4.2 GREEN: add `NOTIFICATION_ACK_WAIT_SECONDS`, use it in the default, the validator and the dispatcher durable check, add the comment to `dispatcher.conf.example` and the sentence to `deploy/README.md`; rerun. Name the break in the commit body.

## 5. Final gate

- [ ] 5.1 Run the repository gate from `DEVELOPMENT.md` and `.github/workflows/ci.yml`, `cargo deny --locked check` and `openspec validate --all --strict`; archive this change when every task above is ticked.
