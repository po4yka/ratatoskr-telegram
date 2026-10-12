## Why

XR-021 round 4 (CONTRACTS.md section R4-08) records three Telegram defects that the round 3 native smoke and the independent verifiers found and that were re-verified against `origin/main`. The idempotency key of a capture is derived from the sender and the content only, while Platform fingerprints the exact request body, so the same PDF sent plain and then forwarded (or a URL sent bare and then forwarded) hits Platform with one key and two bodies, is refused 409, and the sender is told "Could not save this item" although the first send was saved. The webhook capture path never drops a Platform credential that Platform rejected, so a revoked or lost session makes every capture from that sender fail permanently until the cached credential ages out (up to about 55 minutes). `notification_bus.ack_wait_seconds` accepts 1 to 600 although Edge fixes the `ratatoskr_telegram_notifications` durable at 30 seconds and the dispatcher refuses any other value.

## What Changes

- `platform_api::CaptureSubmission` gains `request_parts() -> (&'static str, serde_json::Value)`, the route path and the JSON body, and `Client::submit_capture` serialises exactly that value. The idempotency key becomes `hex(sha256("capture.v2|" || sender || "|" || path || "|" || body bytes))`, derived in `intake/intent.rs` over `request_parts`, so a key can never describe a body other than the one sent. `capture_key` and `blob_capture_key` are deleted and every call site is updated; `retry_key` follows the same derivation.
- `PlatformError::Conflict` from a capture is a transient class: the bounded retry applies and exhaustion answers the temporarily-unavailable reply.
- On `PlatformError::Unauthenticated` the capture path invalidates the cached credential (only when it is still the current entry), exchanges a fresh session and submits once more before classifying a permanent refusal. The extra attempt is separate from the transient attempt bound and covers URL and blob captures.
- `notification_bus.ack_wait_seconds` must equal 30. The value is the public constant `NOTIFICATION_ACK_WAIT_SECONDS` in `crates/core/src/config/model.rs`, used by the default, the validator and the dispatcher durable check. The key stays only to state the value; the validation message names Edge's fixed durable and the dispatcher example says so.

## Capabilities

### Modified Capabilities

- `article-capture`: the idempotency key describes the request body; a conflict is a transient failure; a rejected credential is dropped and the capture retried once.
- `attachment-ingestion`: a PDF sent plain and then forwarded yields two accepted operations.
- `service-configuration`: the notification ack wait is exactly 30.
- `deployment-profile`: the dispatcher example states why the ack wait is 30.

## Impact

- Deliberate break, no shim: every idempotency key changes (`capture.v1` to `capture.v2`, now over the request body). A resend right after the deploy of a link or file that was already captured creates one new operation instead of replaying the old one. Nothing is deployed, so the break is named and not staged.
- Deliberate break: an `RATATOSKR__NOTIFICATION_BUS__ACK_WAIT_SECONDS` other than 30 now stops startup in the dispatcher with a violation naming Edge's durable.
- A rejected credential now costs one extra session exchange and one extra Platform call per rejected capture, and `permanent_refusal_settles_immediately` (a credential refusal) now makes two attempts instead of one.
- Cross-repository behaviour is defined in XR-021 CONTRACTS.md section R4-08, not in this change; this change cites it.
- `schema.sql` is untouched and there is no migration.
