## 1. Contracts crate dependency and blob store

- [x] 1.1 Add `ratatoskr-identifiers` as a git workspace dependency pinned to contracts revision `d56c6891ce9b4fca2c65d43205080d9a666ab5a0` (the revision the extractor pins), satisfying `deny.toml`'s `required-git-spec = "rev"`. Cannot start from a failing test: dependency wiring only, no behavior to assert.
- [x] 1.2 RED: add `crates/blob-store/src/lib.rs` unit tests `storing_bytes_publishes_the_fleet_blob_ref_and_content_path`, `identical_bytes_converge_on_one_reference_without_duplication`, `verify_accepts_its_own_store_and_rejects_tampering` (flip one stored byte → verify fails on digest; wrong owner/media/length → verify fails), each asserting the BlobRef fields (owner `ratatoskr-telegram`, sha256 hex of the bytes, media type, length) and the `sha256/<2>/<62>` path layout under a tempdir root; confirm they fail because no store exists — confirmed: both store tests failed with `Mismatch("no store is implemented yet")`, the verify test failed for the same absence before its own assertions could run
- [x] 1.3 GREEN: implement the store - streaming SHA-256 into a staging file, hard-link (rename fallback) to the content-addressed path, `verify` over owner/algorithm/digest/media-type/length; rerun until 1.2 passes — 3/3 green

## 2. Bot API file retrieval and bounded streaming download

- [x] 2.1 RED: extend `crates/bot-api/tests/client.rs` harness with raw-byte responses plus a `get_me.json`-style `get_file.json` fixture, then add `get_file_resolves_the_harness_file_metadata`: calling `get_file` for a synthetic `file_id` records `POST /bot{token}/getFile` carrying that id and resolves the served `file_path`; confirm it fails on the missing method — confirmed: `Api { "get_file is not implemented yet" }`
- [x] 2.2 GREEN: implement `get_file`; rerun until 2.1 passes — package green 18/18
- [x] 2.3 RED: add `download_streams_exactly_the_served_bytes_and_reports_progress`: consuming the bounded download against a harness route serving known bytes yields those exact bytes through the stream without buffering the whole response first; confirm it fails on the missing method — confirmed: `Api { "download_file is not implemented yet" }`
- [x] 2.4 GREEN: implement the streaming download over reqwest inside the client boundary; rerun until 2.3 passes
- [x] 2.5 RED: add `download_aborts_when_the_stream_passes_the_byte_budget`: a harness serving more bytes than the budget makes the read fail with the budget-exceeded class once the budget is crossed; confirm it fails while no budget check exists — rehomed per design: the authoritative counter lives in the blob store (it already counts and hashes every byte), so this pair runs as blob-store unit tests `a_stream_overrunning_the_budget_aborts_without_publishing` / `bytes_exactly_within_the_budget_store_normally`; RED confirmed with the signature-only stub: the overrun store succeeded and published, failing the abort assertion
- [x] 2.6 GREEN: enforce the cumulative byte counter in the streaming reader; rerun until 2.5 passes — blob-store package green 5/5 with enforcement live

## 3. Configuration

- [x] 3.1 RED: add `crates/core/tests/ingestion_config.rs::ingestion_budget_parses_defaults_and_named_bounds`: absent key loads the documented default; an in-range value loads exactly; zero or above-ceiling values are refused naming key, env var, and bound without quoting the value; unknown ingestion fields are refused; confirm it fails on the missing section — confirmed: the refusal test failed with no V18 wired (load succeeded); the defaults/parse and unknown-field cases passed as contract guards from their first run
- [x] 3.2 GREEN: add the `RATATOSKR__INGESTION__MAX_ATTACHMENT_BYTES` section (default 18 MiB, ceiling 20 MiB, validation rule V18) with `.env.example` documentation; rerun until 3.1 passes — core package green 38/38

## 4. Schema and persistence

- [x] 4.1 RED: add `crates/persistence/tests/intents.rs::intents_carry_bounded_metadata_and_optional_source_address`: fresh schema exposes nullable `source_url` plus object-typed `metadata jsonb`; inserting an attachment intent (null address + blob metadata) succeeds, a URL intent still inserts, and a row with neither is refused by the table constraint; round-trip reads return both shapes; confirm it fails against the current schema
- [x] 4.2 GREEN: edit `schema.sql` in place (`source_url` nullable, `metadata jsonb`, CHECK pairing them) and widen `NewIntent`/`IntentRecord` with the typed metadata shape; rerun until 4.1 passes — package green 38/38. Note: the pairing constraint is written strictly boolean (`coalesce(jsonb_exists(metadata,'blob'), false)`); a plain nullable `or` evaluates NULL when both sides are unknown and PostgreSQL accepts NULL checks, which the test caught
- [x] 4.3 RED: add `forward_origin_metadata_round_trips_through_the_persistence_boundary` pinning the minimized origin facts (kind vocabulary user/hidden_user/chat/channel, identifiers, original date) survive insert/read unchanged and reject unknown members; confirm it fails before the serde shape exists
- [x] 4.4 GREEN: implement the typed metadata value shared by webhook and dispatcher crates; rerun until 4.3 passes

## 5. Platform capture source and origin wire values

- [x] 5.1 RED: extend `crates/platform-api/tests/client.rs` with `submit_capture_posts_blob_sources_and_additive_origin`: a blob-source submission records body `{"blob":{...},"media_type":...}` (no `url` member) and a URL submission with provenance records `{"url":...,"origin":{...}}`; confirm it fails because only `{url}` exists — confirmed as two focused tests, both failing against the URL-only stub (`{"url":""}`; missing origin member)
- [x] 5.2 GREEN: model the typed source enum and optional origin on the submission path keeping URL-only bodies byte-compatible when provenance is absent; rerun until 5.1 passes — package green 8/8. Note: origin travels as pre-serialized JSON from the caller for now; it tightens to a typed Platform contract with the cross-repo changeset named in the proposal

## 6. Forward provenance propagation

- [x] 6.1 RED: add `services/webhook/tests/capture.rs::forwarded_message_with_link_submits_capture_with_origin`: a synthetic forwarded-channel update carrying a link produces a Platform submission whose body carries the URL plus origin facts (kind, identifiers, original date), persists the intent row with the same metadata, derives the ordinary URL idempotency key, and sends exactly one acknowledgment; confirm it fails because forwards settle unsupported today
- [x] 6.2 GREEN: extend message classification (`MessageParts` origin extraction from text/caption) and thread provenance into intent + submission; rerun until 6.1 passes
- [x] 6.3 RED: add `first_forwarded_link_wins_and_linkless_forwards_stay_unsupported`: two links in a caption submit only the first; a forward with no link settles unsupported with no outbound traffic; confirm it fails before the grammar covers captions
- [x] 6.4 GREEN: implement first-link selection and linkless-forward settlement; rerun until 6.3 passes

## 7. Attachment ingestion end-to-end

- [x] 7.1 RED: add `services/webhook/tests/capture.rs::pdf_document_within_limits_stores_and_submits_a_blob_capture`: a synthetic document update drives getFile + raw-byte harness routes, stores into a tempdir-rooted blob store, and asserts the submitted body references the stored BlobRef (owner, digest of the served bytes, media type, length) with no fabricated URL, the intent row carries blob metadata with null address, and one bound ack job exists; confirm it fails because documents are unsupported today
- [x] 7.2 GREEN: wire the attachment branch - allowlist gate, budget gate on declared size, download into the store via the bot-api seam, blob capture submission, binding/intent/ack identical to URLs; rerun until 7.1 passes
- [x] 7.3 RED: add `photo_attachments_ingest_like_documents_with_largest_size_within_budget` and `oversized_declared_size_is_refused_before_any_download`: the photo case picks the largest size and completes a blob capture; the oversized case enqueues exactly one refusal reply and records zero file-transfer requests at the harness; confirm both fail before the gates exist
- [x] 7.4 GREEN: implement photo selection and the declared-size refusal reply; rerun until 7.3 passes
- [x] 7.5 RED: add `a_stream_overrunning_the_budget_fails_the_update_without_publishing_a_blob`: declared size within limits but served bytes beyond the budget settle failed with the safe class and leave no published blob or capture submission; confirm it fails before the reader counter guards the pipeline
- [x] 7.6 GREEN: connect the budget abort to settlement and skip publication of partial bytes; rerun until 7.5 passes

## 8. Unsupported-type truthfulness

- [x] 8.1 RED: add `services/webhook/tests/capture.rs::unsupported_media_gets_one_explicit_truthful_reply`: synthetic voice, video, and unlisted-MIME document updates each produce exactly one HTML reply stating the type is not supported yet, with no session exchange or capture call recorded at the fake Platform, and the update settling without a fabricated success; confirm it fails because such updates currently vanish silently
- [x] 8.2 GREEN: implement the explicit-reply branch for unsupported media kinds and MIME types; rerun until 8.1 passes

## 9. Terminal renders for URL-less captures

- [x] 9.1 RED: add `services/dispatcher/tests/payload.rs::attachment_terminal_describes_media_without_fabricating_a_link`: a succeeded terminal event whose intent metadata carries blob facts composes status lead, media type and size description, the deep-link button, and no hyperlink; confirm it fails because composition requires a source address
- [x] 9.2 GREEN: branch terminal composition on metadata blob facts while leaving URL captures byte-identical; rerun until 9.1 passes

## 10. Documentation and status

- [x] 10.1 Update `README.md` (status paragraph, article workflows, data ownership note), `DEVELOPMENT.md` (current-stage, new config keys, database-recreate reminder), and `docs/DATA_MODEL.md` intents description. Cannot start from a failing test: documentation.
- [x] 10.2 Run the full gate list from DEVELOPMENT.md against a recreated local database and record the evidence in this change before archive — 2026-08-27 green against an isolated PostgreSQL 17 instance.
