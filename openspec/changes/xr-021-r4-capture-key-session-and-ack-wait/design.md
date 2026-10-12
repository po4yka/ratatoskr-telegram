## Context

XR-021 CONTRACTS.md section R4-08 is the single source of truth. This document records only decisions internal to this repository.

## Decisions

### The request parts are the only source of both the body and the key

`CaptureSubmission::request_parts()` returns the route path and the `serde_json::Value` body (the `url` or `blob` member plus `origin` when present). `Client::submit_capture` posts that value and nothing else, and `intake/intent.rs` hashes `capture.v2|<sender>|<path>|<serde_json::to_vec(body)>`. `serde_json::Value` objects are ordered maps with deterministic key order, and `reqwest`'s `.json()` serialises the same value with the same `serde_json` writer, so the bytes hashed are the bytes sent. The constructor in `intent.rs` builds the submission with an empty key, derives the key over its parts, and returns the submission with the key set, so no second place restates the body.

### The same body converges; another body is another operation

A double send of one message produces one body and therefore one key, which Platform replays. A forward of the same PDF (it adds `origin`), or the same bytes under another media type, produces another body and another key, which Platform treats as a separate operation over the same content. The extractor's source identity is content-addressed per owner, so one document results. That is the contract's choice and this change does not try to detect the duplicate on the Telegram side.

### A conflict is transient

With the key tied to the body, Platform can answer 409 only for a request that is still in flight. The existing bounded retry (two attempts, 250 ms apart) then covers it, and exhaustion answers the temporarily-unavailable reply, which tells the truth: the capture was not confirmed and may be resent.

### The credential retry is an attempt of its own

`capture::submit` runs the transient-bounded submit loop. When it ends in `PlatformError::Unauthenticated`, `SessionSource::invalidate_credential(subject, rejected)` drops the entry only if it is still the one that was rejected (a concurrent exchange may already have replaced it), a fresh credential is fetched, and the loop runs once more. A second `Unauthenticated` is a permanent refusal. The retry is a single extra pass, not part of `SUBMIT_ATTEMPTS`, so a transient outage keeps its two-attempt bound and a credential problem costs one exchange. The submit function takes the retry path for both source kinds because both go through it.

### The ack wait constant is shared by three readers

`NOTIFICATION_ACK_WAIT_SECONDS` lives in `crates/core/src/config/model.rs`. The default function returns it, `validate.rs` compares against it, and the dispatcher's durable check reads the configured value that validation has already pinned to it. A workspace harness check can therefore scan one literal against Edge's durable table. The configuration key remains so an operator sees the value and so unknown-key refusal stays strict, which is the pattern Knowledge adopted in R3-09.

## Risks

- The new key derivation changes every key once. Documented as a break; no data outlives it because operations are not keyed by Telegram after acceptance.
- A 403 is also mapped to `Unauthenticated`; it now causes one exchange and one more attempt before the permanent refusal. That is bounded and the second answer decides.
