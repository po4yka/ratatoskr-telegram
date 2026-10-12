## Why

XR-021 round 5 (CONTRACTS.md section R5-10) records that the Platform client builds one `reqwest::Client` with the configured whole-call timeout (`platform.timeout_seconds`, default 10 s) and sends the Server-Sent Events request of the dispatcher's progress follower through it. reqwest's total timeout covers the response body, so every stream is cut at about 10 s although Platform holds it open with 15 s keep-alives. The follower recovers (the cut counts as a clean close, it reconnects with `Last-Event-ID`, three closes end the follow and the 5 s scan restarts it), but every operation that outlives 10 s reconnects about every 12 s, logs `follow_frame_failed` warnings and spends the owner's 120 requests per minute.

## What Changes

- `platform_api::Client` keeps its whole-call client for every request/response call and gains a second client for the stream, built with `connect_timeout` equal to the configured call timeout, `read_timeout` equal to `PLATFORM_STREAM_IDLE_TIMEOUT` (45 s, three keep-alive intervals, a public constant) and no total timeout. `stream_events` uses it and keeps the `classify` mapping.
- `Client::with_timeouts(base_url, call, idle)` is the constructor that takes both values; `Client::new` calls it with the constant. Tests inject a short idle value through it.
- A stream that receives no byte for the idle bound ends as a clean close, as a cut stream does today.
- `README.md` and `deploy/systemd/dispatcher.conf.example` state that `platform.timeout_seconds` bounds request/response calls and not the progress stream, and give the 45 s idle bound.
- The dispatcher follower scan, its three-closes bound and the 5 s scan restart are unchanged. The Bot API client keeps its own whole-call timeout; its download speed limit stays recorded open (R5-15).

## Capabilities

### Modified Capabilities

- `operation-projection`: the progress stream is bounded by an idle timeout, not by the whole-call timeout.
- `deployment-profile`: the dispatcher example and the README state which timeout bounds which call.

## Impact

- Behaviour change, no shim: a long-running operation is followed over one connection instead of one per 12 s, so the owner's request budget is no longer spent on reconnects. A Platform that goes silent for 45 s ends the stream (a clean close) instead of 10 s.
- `Client::new` keeps its signature; `Client::with_timeouts` is added.
- Cross-repository behaviour is defined in XR-021 CONTRACTS.md section R5-10, not in this change; this change cites it.
- `schema.sql` is untouched and there is no migration.
