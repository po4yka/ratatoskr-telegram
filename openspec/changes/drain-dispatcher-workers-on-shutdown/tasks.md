## 1. Owned background lifecycle

- [x] 1.1 RED: add crates/http/tests/background.rs with shutdown_signals_and_joins_background_before_returning using oneshot gates instead of sleeps; predict the current background factory cannot expose a joinable task and shutdown returns while the fake task is live, run only this test through build-gate -- cargo nextest run --locked -p ratatoskr-telegram-http shutdown_signals_and_joins_background_before_returning, and confirm the behavioral assertion failure after any minimal compiling seam
- [x] 1.2 GREEN: introduce an owned background runtime carrying root cancellation plus supervised handles, have HTTP shutdown signal and join it before shared resource close, and rerun the exact test until green
- [x] 1.3 RED: add drain_deadline_aborts_and_awaits_stuck_background; gate a child beyond the deadline, predict current lifecycle has no bounded abort-and-await path, run the exact test through the build gate, and confirm the expected assertion failure with paused time
- [x] 1.4 GREEN: implement bounded drain, abort, and await semantics including escalation from a second shutdown signal, then rerun the exact test until green

## 2. Dispatcher worker ownership

- [x] 2.1 RED: add services/dispatcher/tests/runtime.rs with shutdown_stops_new_claims_and_waits_for_inflight_delivery; gate the first fake Bot API request, signal shutdown, make a second job ready, and predict current detached workers either claim the second or outlive shutdown; run only this test through build-gate -- cargo nextest run --locked -p ratatoskr-telegram-dispatcher shutdown_stops_new_claims_and_waits_for_inflight_delivery and confirm the predicted failure
- [x] 2.2 GREEN: register sender, projection consumer, follower, and notification workers under one supervisor; add cancellation-before-next-claim or fetch arms and preserve the admitted delivery critical section through its durable outcome boundary, then rerun the exact test until green
- [x] 2.3 RED: add production_runtime_owns_all_four_worker_roles plus HTTP resource-order gates;
  predict the detached composition cannot return or count owned sender, projection, follower, and
  notification handles and shutdown returns before gated background completion, then run the
  targeted tests through the build gate and confirm the missing ownership/lifecycle seam
- [x] 2.4 GREEN: register all four roles, synchronously seal producer admission, drain the feed,
  join children before database/telemetry close under the common deadline, and rerun the targeted
  tests until green

## 3. Regression gate

- [x] 3.1 REFACTOR: remove detached spawn paths and duplicate cancellation plumbing, then run HTTP and dispatcher crate suites through the build gate and confirm every spawned test handle is awaited
- [x] 3.2 Run the complete DEVELOPMENT.md gate, strict validation for this change, and diff review for unbounded waits, cancellation races, or unrelated behavior
- [x] 3.3 Mark tasks complete only after observed checks, stage only this change's paths, inspect the staged diff, and create its dedicated main-branch commit
