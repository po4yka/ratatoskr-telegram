## 1. Retry live followers without restart

- [x] 1.1 RED: add temporary_session_exchange_failure_is_retried_while_binding_is_live to services/dispatcher/tests/follow.rs with a one-shot fake session fault; predict the second scan opens no stream because the operation entered finished, run only this test through build-gate -- cargo nextest run --locked -p ratatoskr-telegram-dispatcher temporary_session_exchange_failure_is_retried_while_binding_is_live, and confirm the open-count assertion fails
- [x] 1.2 GREEN: remove process-lifetime finished authority, clear only in-flight state after a retryable exit, and let a later scan reschedule each durable nonterminal binding; rerun the exact test until green
- [x] 1.3 RED: add three_nonterminal_stream_closes_are_retried_by_a_later_scan with deterministic open notifications and a fake clean-close server; predict stream opens stop at the current per-task bound, run the exact test through the build gate, and observe the expected missing later open
- [x] 1.4 GREEN: retain bounded reconnects per task but return exhausted nonterminal work to scan eligibility with backoff, then rerun the exact test until green

## 2. Fresh session on every stream open

- [x] 2.1 RED: add reconnect_after_session_expiry_uses_a_fresh_credential with recorded credentials and a fake clock; predict reconnect reuses the first credential, run only this test through the build gate, and confirm the credential assertion fails
- [x] 2.2 GREEN: resolve owner and acquire a valid session before every initial open and reconnect, conditionally invalidate only the rejected cached credential on authentication failure, and rerun the exact test until green
- [x] 2.3 RED: add shutdown_cancellation_does_not_mark_a_live_follower_terminal; gate a live stream, cancel its owned worker, predict current unconditional finished bookkeeping suppresses the next scan, run the exact test through the build gate, and confirm the missing restart assertion
- [x] 2.4 GREEN: treat shutdown cancellation as nonterminal, preserve the in-flight insert-before-spawn guard and cleanup under the owned dispatcher supervisor, then rerun the exact test and existing Last-Event-ID replay tests

## 3. Regression gate

- [x] 3.1 REFACTOR: consolidate follower exit classification and safe-class telemetry without adding a second durable state machine, then run the dispatcher crate suite through the build gate
- [x] 3.2 Run the complete DEVELOPMENT.md gate, strict validation for this change, and diff review for unbounded retries, credential logging, leaked tasks, or schema drift
- [x] 3.3 Mark tasks complete only after observed checks, stage only this change's paths, inspect the staged diff, and create its dedicated main-branch commit
