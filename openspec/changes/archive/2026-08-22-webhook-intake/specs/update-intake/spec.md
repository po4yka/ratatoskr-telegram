## Purpose

How a delivered Bot API update is admitted: secret verification before anything else, method and content-type restriction, body and schema limits, persisted deduplication, fast acknowledgment, and one typed outcome per rejection class.

## ADDED Requirements

### Requirement: The webhook secret is verified before any body byte is read or parsed

The webhook route SHALL answer only requests carrying `X-Telegram-Bot-Api-Secret-Token` equal to the configured webhook secret, compared in constant time. A request with a missing or wrong secret SHALL be rejected 401 before the body is read, and SHALL NOT produce a database write.

#### Scenario: A request without the secret header is unauthorized

- **WHEN** a POST arrives without `X-Telegram-Bot-Api-Secret-Token`
- **THEN** the response is 401, nothing is parsed, and no update row exists

#### Scenario: A forged secret is unauthorized

- **WHEN** a POST arrives with a secret header that differs from the configured value
- **THEN** the response is 401 and no update row exists

### Requirement: Method, content type and body size are limited before parsing

The route SHALL accept only POST with an `application/json` content type. A declared `Content-Length` above the configured maximum SHALL be rejected 413 with an explicit limit response before the body is read. A streamed body exceeding the same cap while being read SHALL be cut off and rejected 413. No rejected request SHALL reach schema parsing.

#### Scenario: An oversized declared body is rejected with the limit response

- **WHEN** a POST declares a `Content-Length` greater than `RATATOSKR__WEBHOOK__MAX_BODY_BYTES`
- **THEN** the response is 413 and names the limit, and no update row exists

#### Scenario: A chunked body cannot exceed the cap by lying about its size

- **WHEN** a POST streams more bytes than the configured maximum without declaring them
- **THEN** the read is cut off at the cap, the response is 413, and no update row exists

#### Scenario: A non-JSON content type is refused

- **WHEN** a POST arrives with a content type other than `application/json`
- **THEN** the response is 415 and no update row exists

#### Scenario: A non-POST method is refused

- **WHEN** a GET arrives on the webhook path
- **THEN** the response is 405

### Requirement: Malformed updates are acknowledged to prevent retry storms, unsupported kinds are recorded

An update whose JSON cannot be parsed against the Bot API schema SHALL be answered 200 and logged by safe failure class, without a database write. An update whose envelope parses but whose kind is unknown to this build SHALL be accepted, answered 200, recorded as unsupported, and never treated as malformed.

#### Scenario: A malformed payload is acked, not retried forever

- **WHEN** the body is not valid JSON, or carries no usable `update_id`
- **THEN** the response is 200, the payload class is logged without its content, and no update row exists

#### Scenario: An unknown update kind is supported input this build does not act on

- **WHEN** the body parses as an update but its kind is not one this build handles
- **THEN** the response is 200, exactly one update row exists recording the unsupported kind, and it settles as `unsupported`

### Requirement: Deduplication is exact-match persistence over bot identity and update id

Every admitted update SHALL be recorded in `telegram.updates` keyed by bot identity and `update_id` before any processing handoff. A redelivered update whose pair already exists SHALL be answered 200, counted as deduplicated, and handed to processing exactly once ever. An unseen update id below the highest seen id SHALL still be processed.

#### Scenario: A duplicate delivery has no effect the second time

- **WHEN** the same update is delivered twice with the same secret
- **THEN** both responses are 200, one update row exists, and downstream processing received the update once

#### Scenario: An out-of-order older duplicate is dropped by identity, not by order

- **WHEN** updates arrive in the order 100, 42, 42 where 42 was already delivered before 100
- **THEN** ids 100 and 42 are each processed once, and the second 42 is dropped as a duplicate

#### Scenario: An unseen older id is not confused with a duplicate

- **WHEN** update 100 is delivered and later update 99 arrives for the first time
- **THEN** update 99 is processed like any new update

### Requirement: Acknowledgment precedes all downstream work

The webhook route SHALL return its final status without waiting for any domain work: after admission succeeds the update SHALL be handed to a bounded in-process queue consumed by a worker task, and requests SHALL complete promptly even while the worker is blocked. Queue saturation and storage failure SHALL answer 503 so Telegram retries, with no partial side effect left behind.

#### Scenario: Requests complete while processing is stalled

- **WHEN** the worker task is blocked and several valid updates are delivered
- **THEN** every request answers 200 promptly, and each queued update is processed only after the worker resumes

#### Scenario: A saturated queue refuses without persisting

- **WHEN** the queue has no free capacity when an update arrives
- **THEN** the response is 503, no update row exists for it, and Telegram's retry can succeed later

#### Scenario: A storage failure refuses without persisting

- **WHEN** the database cannot accept the deduplication insert
- **THEN** the response is 503 and no acknowledgment of success was given

### Requirement: Every rejection and acceptance is observable by safe class

Each webhook request SHALL increment a request-outcome counter drawn from a closed vocabulary (`accepted`, `deduplicated`, `unauthorized`, `too_large`, `wrong_media_type`, `method_not_allowed`, `malformed`, `overloaded`) and SHALL be logged by that class without request bodies, secrets, chat identifiers or message text. Admission duration SHALL be recorded on a histogram.

#### Scenario: Outcomes are countable without content

- **WHEN** requests of several outcome classes are served
- **THEN** the counters name each class and no label value contains request-controlled text beyond the closed vocabulary
