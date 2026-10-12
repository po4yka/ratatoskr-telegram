## Context

XR-021 CONTRACTS.md section R5-10 is the single source of truth. This document records only decisions internal to this repository.

## Decisions

### Two clients, one base URL

`Client` holds `http` (whole-call, `timeout(call)`) and `stream_http` (`connect_timeout(call)`, `read_timeout(idle)`, no total timeout). Both use rustls. Every request/response method keeps `self.http`; only `stream_events` takes `self.stream_http`. Splitting by client rather than overriding the timeout per request is deliberate: reqwest's per-request `RequestBuilder::timeout` can only set a total timeout, never remove the client's, so the stream needs a client that never had one.

### The idle bound is a read timeout

reqwest's `read_timeout` is reset after every successful read of the body, so a keep-alive comment every 15 s keeps the stream alive and 45 s of silence (three missed keep-alives) ends it. `PLATFORM_STREAM_IDLE_TIMEOUT` is a public constant so a workspace check or an operator document can name it; it is not configurable, because the interval it covers is fixed by Platform.

### What an idle stream returns

reqwest reports a read timeout through the same `is_timeout` flag as a total timeout, and `EventStream::next_frame` maps it to `PlatformError::Timeout`. The follower already counts that error on an established stream as one clean close, so the idle end is the existing close path and no new class is added. The test asserts the typed `Timeout` that `next_frame` returns, and the doc comment of the variant now names both causes.

### The follower needs no change

The follower reconnects only because the stream errors. With the stream no longer cut, the follow of a 25 s operation is one connection to its terminal frame. The follower test counts the requests at the fake Platform. If the follower still reconnected, the cause would be fixed and named in the commit.

## Risks

- reqwest 0.12 starts the read timeout when the request starts and re-arms it after every received chunk, so it also bounds the wait for the response head; a server that accepts the connection and never answers ends `stream_events` with `PlatformError::Timeout` after the idle bound rather than hanging, because the stream client has no total timeout. `a_stream_whose_head_never_arrives_is_cut_by_the_idle_timeout` pins this.
- The idle end surfaces as `PlatformError::Timeout` from `next_frame`, which the follower already logs as `follow_frame_failed` and counts as one clean close. It is a real fault (Platform went silent), so the warning stays.
