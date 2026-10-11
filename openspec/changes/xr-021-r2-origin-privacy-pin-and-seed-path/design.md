## Context

XR-021 CONTRACTS.md sections R2-17 and R2-10 are the single source of truth for the contracts pin and the seed path. This document records only decisions internal to this repository.

## Decisions

### The origin is derived by one small function beside `submit`

`origin_of(&IntentMetadata) -> Option<serde_json::Value>` returns `{"forward": <forward member>}` when a forward is present and `None` otherwise. The wire key stays `forward`, so the shape Platform and the existing forwarded-URL test see is unchanged; only the sibling `blob` member disappears. Persistence is untouched: the intent row still stores the full `IntentMetadata`, because the terminal render describes the attachment from it.

### The contracts pin is a manifest change gated by the existing suite

No failing test can precede a dependency pin, so the gate is the existing suite staying green on the new revision, including the wire tests of `crates/platform-api` (`assertion.rs`, `client.rs`, `session.rs`). The old and new revisions differ only in additive contract content and one test file, so no source fallout is expected.

### The seed path is asserted from the shipped files

`deployment_profile.rs` reads the dispatcher example and the guide, so the seed path and the install command cannot drift from the fleet convention unnoticed.

## Risks

- An already deployed dispatcher keeps working only after the operator moves the seed file and edits its configuration; that is the named break of this change.
