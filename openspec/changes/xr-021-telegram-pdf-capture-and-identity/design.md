## Context

XR-021 CONTRACTS.md section S11 is the single source of truth for the wire, the static reply bodies, the identity binding hook and the deploy values. This document records only the decisions internal to this repository.

## Decisions

### Failure replies are enqueued by the worker, not by the capture module

`capture::submit` keeps returning a `SubmitClass`. `capture_run.rs` maps `TransientExhausted` and `PermanentRefusal` to a static reply through `enqueue_text`, then settles `Failed`. If the reply enqueue itself errors, the state is still `Failed` (the enqueue result is only logged). `AcceptedProjectionPending` keeps its retry path with no reply because Platform already holds the operation. The metric `TELEGRAM_CAPTURE_SUBMISSIONS_TOTAL{class}` is unchanged.

### The attachment code moves into its own worker module

`worker.rs` is at the 850-line cap. The attachment half (`handle_attachment`, `select_attachment`, the reply kinds and the message-attachment classification) moves to `worker/attachment.rs` in a refactor step that runs after the behaviour is green and adds no test. Photos are classified as unsupported at the message boundary, so the `Photos` variant and `PhotoCandidate` disappear instead of being kept alive for a path that always refuses.

### The identity hook is a post-exchange await on the session source

`SessionSource::with_on_mint(hook)` stores an `Arc<dyn Fn(String, Uuid) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>`. The hook runs once after every successful network exchange, after the cache insert, and cannot fail the exchange because it returns `()`. Cache hits never call it. This binds on the first capture or library command of a sender and again after each credential refresh, which is also how a Platform-side mapping change reaches Telegram (Platform is authoritative, so a changed mapping overwrites). Both roles install the same hook; the dispatcher's `SessionSource` serves the operation follower and notification path.

`Database::bind_internal_user` is a single conditional `update ... where internal_user_id is distinct from $2`, so a repeat is a no-op and a missing identity row updates nothing and is not an error. The hook maps the affected row count to `bound` or `unchanged` and a database error to `failed` plus the log class `identity_binding_failed`; a failed write is retried at the next network exchange.

### The PDF-only rule lives in `select_attachment`

Any document with a MIME type reaches `select_attachment`, which accepts only `application/pdf`. A document without a MIME type, a photo and every other media kind map to the one static unsupported reply. The declared-size limit reply stays and now says "PDF document".

### Deploy values are asserted from the shipped files

`deployment_profile.rs` reads the example files and the unit, so the audience, base URL, blob root, `UMask`, `SupplementaryGroups` and README install step cannot drift from the contract unnoticed. The compiled defaults are pinned by `platform_config.rs`.

## Risks

- The blob store is content-addressed and shared across users; the guard is Platform's Telegram-only session kind, the owner allowlist and the unguessable 256-bit digest (S11).
- A missing `ratatoskr-telegram-blobs` group is host state; the extractor reports `blob_unreadable`.
- The live composed Telegram to Edge to Extractor path is not exercised here (workspace wave 3).
