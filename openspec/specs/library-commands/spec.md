# library-commands Specification

## Purpose

Adapts Platform's authoritative library search and read-state resources into bounded, private, replay-safe Telegram commands without moving Knowledge data ownership into the bot.

## Requirements

### Requirement: Library commands parse deterministically after authorization

An enabled actor in a bound private chat SHALL be able to send `/search <query>`, `/unread`, or `/read <token>`. A search query SHALL contain 1 through 256 Unicode scalar values after trimming; `/unread` SHALL contain no argument; `/read` SHALL contain exactly one 64-character URL-safe opaque token. Invalid forms SHALL enqueue bounded usage guidance and SHALL perform no Platform library call. Non-private and unauthorized updates SHALL remain governed by the existing denial path and SHALL not reveal that the library capability exists.

#### Scenario: Exact command forms are accepted

- **WHEN** an enabled private-chat actor sends `/search durable queues`, `/unread`, and `/read <valid-token>` as separate updates
- **THEN** each update reaches its corresponding typed library intent and no other command handler claims it

#### Scenario: Invalid forms make no Platform call

- **WHEN** an actor sends `/search` without text, `/unread extra`, or `/read` with a malformed token
- **THEN** one usage reply is queued for each update and the Platform harness records no library request

### Requirement: Search and unread use bounded Platform queries

The post-ack worker SHALL call Platform, never Knowledge, with a finite timeout. `/search <query>` SHALL request offset zero, limit five, and no read-state filter; `/unread` SHALL request offset zero, limit five, a blank query, and filter `unread`. The worker SHALL first require the corresponding Platform capability and SHALL make no library request while it is absent.

#### Scenario: Search and unread map exactly

- **WHEN** the actor sends `/search recovery` and then `/unread` while both capabilities are available
- **THEN** the Platform harness records one five-result query for `recovery` and one five-result blank query filtered to `unread`

#### Scenario: Capability absence refuses locally

- **WHEN** `library.search` is absent from the current Platform capability document
- **THEN** `/search recovery` queues the stable feature-unavailable reply and no search request is sent

### Requirement: Results render safely within one Telegram message

A successful search or unread page SHALL enqueue exactly one direct-response outbound job containing escaped Telegram HTML and at most five results. Each result SHALL include at most 160 Unicode scalar values of title, its effective read state, and at most 320 Unicode scalar values of only the snippet or match information actually supplied by Platform. The complete body SHALL remain shorter than 4096 Unicode scalar values through deterministic per-field and whole-message truncation. Empty pages SHALL render a distinct no-results response.

#### Scenario: Hostile long fields remain inert and bounded

- **WHEN** Platform returns oversized titles and snippets containing `<`, `>`, `&`, and quote characters
- **THEN** the queued HTML is escaped, deterministically truncated, below the Bot API limit, and contains no injected tag

#### Scenario: Empty unread page is explicit

- **WHEN** Platform returns a successful empty unread page
- **THEN** Telegram queues one `No unread items.` response and issues no read token

### Requirement: Unread results receive opaque read commands

When `library.read_state` is available, Telegram SHALL create one server-side read authority for each rendered unread result, expiring 15 minutes after issue, and render only `/read <token>`. When that capability is absent, results SHALL remain readable but carry no read authority. The token SHALL be 64 URL-safe characters, contain no target or content data, and bind the action to the bot, Telegram actor, internal user, and chat. Read results SHALL not receive another read token.

#### Scenario: Rendered token discloses no target

- **WHEN** an unread result contains analysis, document, title, snippet, and tenant values
- **THEN** its rendered token matches the opaque grammar and contains none of those values

#### Scenario: Read capability is absent

- **WHEN** search succeeds while `library.read_state` is absent from the capability document
- **THEN** Telegram renders the results without `/read` tokens and does not claim that marking read is available

### Requirement: Read mutations are authoritative and truthful

After winning one token consumption, the worker SHALL call Platform's idempotent read-state resource with `read`, using bounded retry only for retryable failures. Telegram SHALL queue success only after Platform returns authoritative `read`. Scoped absence SHALL produce item-unavailable guidance, dependency failure SHALL produce feature-unavailable guidance, and exhausted attempts after an uncertain send SHALL say the outcome is unknown and direct the user to `/unread`.

#### Scenario: Successful read is reported after authority answers

- **WHEN** Platform returns `read` for the token-bound analysis
- **THEN** exactly one success reply is queued and a replay of the token performs no second mutation

#### Scenario: Lost responses do not fabricate success

- **WHEN** every bounded attempt loses its response after the request may have reached Platform
- **THEN** Telegram queues outcome-unknown guidance mentioning `/unread` and does not claim the item is read

### Requirement: Library command telemetry contains no library content

The service SHALL count command outcomes using only finite command and outcome classes and SHALL correlate failures with existing safe request/update identifiers. Query text, titles, snippets, tokens, Telegram identifiers, internal users, tenants, document identifiers, and analysis identifiers SHALL NOT appear in ordinary log fields or metric labels.

#### Scenario: A failed query remains content-free in telemetry

- **WHEN** `/search private phrase` receives a Platform timeout
- **THEN** captured ordinary logs and metric labels contain the search/timeout classes but none of `private phrase` or returned library content
