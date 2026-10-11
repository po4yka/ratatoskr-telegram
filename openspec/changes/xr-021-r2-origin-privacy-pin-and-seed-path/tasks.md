Environment for every command: `export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`, `TELEGRAM_TEST_DATABASE_URL` on the private PostgreSQL and `TELEGRAM_TEST_NATS_URL` on the private NATS (XR-021 local ports 57840..57859).

## 1. Only the forward member crosses the Platform boundary (CONTRACTS R2-17)

- [x] 1.1 RED: add `services/webhook/tests/capture.rs::a_forwarded_pdf_sends_only_the_forward_member_as_origin` (the fake Platform captures the body of `POST /v1/captures/blobs`: with a forward present `origin` holds the forward fields and none of `owner_service`, `digest`, `media_type`, `length_bytes` or `blob`; a forward-less PDF sends no `origin`; the `blob` member is unchanged; the intent row still stores the full metadata); run it and confirm it fails because `origin` carries the attachment facts.
- [x] 1.2 GREEN: add `origin_of(&IntentMetadata) -> Option<serde_json::Value>` beside `submit` in `services/webhook/src/intake/capture.rs` and use it; no persistence change; rerun the test and the existing capture tests.

## 2. Align the contracts pin (CONTRACTS R2-17)

- [x] 2.1 A dependency pin cannot start from a failing test; the gate is the existing suite staying green. Move the four `ratatoskr-*` git dependencies to `ad16855c4e7f3d52cd118274faa3b8f3ab4da576`, refresh `Cargo.lock`, fix additive fallout, and run `cargo test --workspace --locked` including the `crates/platform-api` wire tests (`assertion.rs`, `client.rs`, `session.rs`).

## 3. Seed path convention (CONTRACTS R2-10)

- [x] 3.1 RED: add `services/webhook/tests/deployment_profile.rs::dispatcher_example_installs_the_seed_where_the_fleet_does`; run it and confirm it fails on `/etc/ratatoskr/secrets/telegram.nkey`.
- [x] 3.2 GREEN: edit `deploy/systemd/dispatcher.conf.example` and `deploy/README.md`; rerun the deployment profile tests.

## 4. Final gate

- [ ] 4.1 Run the repository gate from `DEVELOPMENT.md` and `.github/workflows/ci.yml`, `cargo deny --locked check` and `openspec validate --all --strict`; archive this change when every task above is ticked.
