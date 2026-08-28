## 1. Typed Platform library client and command routing

- [x] 1.1 RED: add `services/webhook/tests/library_commands.rs::search_and_unread_map_to_bounded_platform_queries_after_ack` using the real intake harness and fake Platform capabilities/search routes, run it, and confirm both updates settle unsupported and the harness records no library queries.
- [x] 1.2 GREEN: add strict capability/library types and methods in `crates/platform-api`, implement exact `/search` and `/unread` parsing/routing after authorization and before fallback, and use the existing session issuer from the post-ack worker; rerun the focused test and verify limit five, offset zero, correct query/filter, and no request before acknowledgment.
- [x] 1.3 RED: extend `services/webhook/tests/library_commands.rs::invalid_library_forms_and_absent_capabilities_never_query_platform` with empty/oversized search, extra unread arguments, malformed read token, and absent capability cases; run it and confirm current parsing or capability handling sends a request or wrong response.
- [x] 1.4 GREEN: implement Unicode bounds, exact token grammar, stable usage/unavailable responses, and per-command capability gating; rerun the focused test and verify no domain request occurs for every refusal.

## 2. Opaque read authority and bounded rendering

- [x] 2.1 RED: add `crates/persistence/tests/interaction_state_schema.rs::library_read_tokens_have_action_specific_command_scope` applying `schema.sql` and asserting `command`/`library_read`/`analysis_id`/`internal_user_id` constraints plus forbidden operation/dialogue payload combinations; run it and confirm schema creation or inserts fail for the missing action shape.
- [x] 2.2 GREEN: edit the current `schema.sql` in place and extend persistence types/issuance/consumption/cleanup for 15-minute bot/actor/internal-user/chat-bound single-use library read authority; rerun schema, token, concurrency, scope, and cleanup tests and verify no migration file is added.
- [x] 2.3 RED: add `services/webhook/tests/library_commands.rs::result_render_is_escaped_bounded_and_issues_only_owner_scoped_read_tokens` with five hostile oversized results plus a read result, run it, and confirm the current worker cannot produce one safe reply with valid tokens and no token for the read item.
- [x] 2.4 GREEN: implement deterministic HTML field/whole-message budgets and one transactional token-plus-direct-outbound enqueue path; rerun the focused test and verify the payload is under 4096 characters, escaped, contains at most five results, and every rendered token resolves only in its owner scope.

## 3. Authoritative read outcomes and privacy telemetry

- [x] 3.1 RED: add `services/webhook/tests/library_commands.rs::read_command_has_one_winner_and_reports_success_not_found_unavailable_and_unknown_truthfully` covering concurrent/replayed token presentation and fake Platform success, scoped absence, dependency failure, and lost-response exhaustion; run it and confirm the command cannot meet the outcome assertions.
- [x] 3.2 GREEN: consume library read authority once, call the idempotent Platform PUT with bounded retry classes, and enqueue the specified truthful replies; rerun the focused test and verify one mutation winner, no false success, and `/unread` reconciliation guidance after uncertainty.
- [x] 3.3 RED: add `services/webhook/tests/library_commands.rs::library_telemetry_contains_only_command_and_outcome_classes`, capture logs/metrics for a query timeout and hostile result, and confirm content or missing class signals fail the assertions.
- [x] 3.4 GREEN: add finite command/outcome counters and safe correlation logging without query/result/token/identity fields; rerun telemetry tests and verify all prohibited values are absent.

## 4. Documentation and gate

- [x] 4.1 Update `/help`, README, architecture/interface/data-retention documentation for exact `/search`, `/unread`, `/read <opaque-token>` semantics and limitations; cannot start from a failing behavior test because this is documentation/help copy, so verify command strings and bounds with focused static assertions.
- [x] 4.2 Run the complete fenced gate in `DEVELOPMENT.md`, including real disposable PostgreSQL tests, fake Platform/Bot API suites, strict OpenSpec validation, and release build limits; verify every command passes before marking implementation complete.
