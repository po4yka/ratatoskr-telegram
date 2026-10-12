Environment for every command: `export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`, `TELEGRAM_TEST_DATABASE_URL` on the private PostgreSQL and `TELEGRAM_TEST_NATS_URL` on the private NATS (XR-021 local ports 57840..57859). The authorized-broker test needs `nats-server` on `PATH`.

## 0. Plan and pin

- [x] 0.1 Open this change (proposal, design, spec deltas, tasks), citing XR-021 CONTRACTS.md R5-10, and run `openspec validate --all --strict`. Documentation, so it cannot start from a failing test.
- [x] 0.2 A dependency pin cannot start from a failing test; the gate is the existing suite staying green. Move the four `ratatoskr-*` git dependencies to `d98967b7b7ee80442c93ef4657140068955c8f61`, refresh `Cargo.lock`, fix additive fallout, and run `cargo test --workspace --locked`.

## 1. The stream client (CONTRACTS R5-10 rules 1 and 3)

- [ ] 1.1 RED: add `crates/platform-api/tests/client.rs` cases for a keep-alive stream that outlives a 500 ms call timeout and delivers its terminal frame over one connection, a silent stream that ends after an injected idle timeout, a stalled request/response body that is still cut by the call timeout, and `Last-Event-ID` on resume; with a signature-only `Client::with_timeouts`, run them and confirm the first fails on its assertion (a timeout and more than one connection today).
- [ ] 1.2 GREEN: add `PLATFORM_STREAM_IDLE_TIMEOUT`, the stream client built with `connect_timeout` and `read_timeout`, make `Client::new` call `with_timeouts`, and send `stream_events` through the stream client; rerun.

## 2. The follower (CONTRACTS R5-10 rule 2)

- [ ] 2.1 RED: add the `services/dispatcher/tests/follow.rs` case where an operation stays queued for several call timeouts and is followed over one connection with no `follow_frame_failed` log and exactly the requests of one follow; run it and confirm it fails on the reconnects.
- [ ] 2.2 GREEN: nothing beyond task 1 is expected; if the follower still reconnects, fix the cause and name it; rerun.

## 3. Documentation

- [ ] 3.1 State in `README.md` and `deploy/systemd/dispatcher.conf.example` that `platform.timeout_seconds` bounds request/response calls and not the progress stream, and the 45 s idle bound. Documentation, so it cannot start from a failing test.

## 4. Final gate

- [ ] 4.1 Run the repository gate from `DEVELOPMENT.md` and `.github/workflows/ci.yml`, `cargo deny --locked check` and `openspec validate --all --strict`; archive this change when every task above is ticked.
