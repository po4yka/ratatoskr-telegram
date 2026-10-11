Environment for every command: `export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`, `TELEGRAM_TEST_DATABASE_URL` on the private PostgreSQL and `TELEGRAM_TEST_NATS_URL` on the private NATS (XR-021 local ports 57840..57859). The authorized-broker test needs `nats-server` on `PATH`.

## 1. Inbox prefix (CONTRACTS R3-01)

- [x] 1.1 RED: add `services/dispatcher/tests/inbox_prefix.rs::the_notification_consumer_completes_jetstream_requests_under_its_inbox_prefix` (a broker whose only user is the stanza of `deploy/nats/identity.conf`, an admin fixture connection creating `ratatoskr_events` and the durable, then `open_preprovisioned_consumer_at`, one published message fetched and acknowledged); run it and confirm it fails because the request times out under the default inbox prefix. The stanza file is a configuration input the test needs to start, so it is added with the test.
- [x] 1.2 GREEN: add `const INBOX_PREFIX: &str = "_INBOX_telegram"` applied with `custom_inbox_prefix`, add the CI step that puts `nats-server` on `PATH`, say so in `DEVELOPMENT.md`; rerun the test and the existing notification tests. Name the break in the commit body.

## 2. The assertion key command (CONTRACTS R3-12 rules 1 and 2)

- [x] 2.1 RED: add `services/webhook/tests/assertion_key.rs` (`public_prints_the_rfc_8032_vector`, `generate_writes_a_private_seed_and_prints_its_public_key`, `public_refuses_a_malformed_seed_without_echoing_it`, `an_unknown_subcommand_is_refused`) and the unit test `crates/platform-api` `the_public_key_matches_the_signer`, with a signature-only `assertion_key` module returning neutral values; run them and confirm they fail on their assertions.
- [x] 2.2 GREEN: implement `generate`, `public_key_base64` and the shared `decode_seed` in `crates/platform-api/src/assertion_key.rs`, have both service builds call it, add the `assertion-key` arm to `services/webhook/src/main.rs`; rerun.

## 3. The procedure (CONTRACTS R3-12 rule 3)

- [x] 3.1 RED: add `services/webhook/tests/deployment_profile.rs::the_deploy_readme_documents_the_assertion_key_procedure`; run it and confirm it fails on the missing section.
- [x] 3.2 GREEN: add the "Platform assertion key" section to `deploy/README.md` and a pointer in each example; rerun.

## 4. Provisioning (CONTRACTS R3-11)

- [x] 4.1 RED: add `services/webhook/tests/deployment_profile.rs::the_role_file_creates_the_role_both_examples_name`; run it and confirm it fails because `deploy/postgres/role.sql` does not exist.
- [x] 4.2 GREEN: add `deploy/postgres/role.sql` and the provisioning-order paragraph in `deploy/README.md`; rerun.

## 5. Final gate

- [ ] 5.1 Run the repository gate from `DEVELOPMENT.md` and `.github/workflows/ci.yml`, `cargo deny --locked check` and `openspec validate --all --strict`; archive this change when every task above is ticked.
