## Purpose

Turns an authorized canonical GitHub repository URL into a safe preview and owner-bound confirmation flow whose final message preserves GitHub Catalog's component-level result exactly.

## ADDED Requirements

### Requirement: Canonical repository URLs render a GitHub preview card

An authorized private-message URL SHALL route to GitHub preview only when its normalized form is exactly `https://github.com/<owner>/<repository>` with an optional trailing slash and no credentials, port, query, fragment, `.git` suffix, or sub-resource path. Telegram SHALL request the preview through its authenticated Platform session and SHALL render escaped repository name, optional description, star count, optional primary language, and only the action buttons reported available by GitHub. The preview itself SHALL cause no action submission.

#### Scenario: Repository URL renders the preview fields

- **WHEN** the GitHub harness returns name `owner/repository`, description `A <tool>`, 42 stars, and language `Rust`
- **THEN** Telegram enqueues one escaped preview card containing those values and available `metadata`, `track`, and `star` selection buttons

#### Scenario: Repository preview does not submit an action

- **WHEN** the preview card is rendered and the user presses no button
- **THEN** the harness records one preview call and zero action calls

### Requirement: Every mode selection requires a second explicit confirmation

Each preview mode button SHALL carry only an opaque one-time selection token. Consuming a valid selection token SHALL render a confirmation prompt that names the exact mode and its effects and SHALL mint distinct opaque confirm and cancel tokens. Only consuming the valid confirm token SHALL submit the action; choosing cancel SHALL terminate the flow with no action. Telegram SHALL answer every recognized callback query promptly while domain work continues.

#### Scenario: Selecting star does not write

- **WHEN** the owner presses the preview's `star` selection button but has not pressed the confirmation button
- **THEN** Telegram answers the callback and renders the provider-write confirmation prompt while the GitHub harness records zero action calls

#### Scenario: Confirmed metadata action submits once

- **WHEN** the owner consumes the metadata selection token and then its valid confirm token
- **THEN** exactly one metadata action is submitted with that flow's stable idempotency key and confirmation evidence reference

#### Scenario: Cancellation performs no action

- **WHEN** the owner consumes the cancel token from a confirmation prompt
- **THEN** the flow becomes cancelled, the callback is answered, and no action request is submitted

### Requirement: Callback tokens are opaque, expiring, owner-bound, and replay-safe

Callback flow state SHALL bind bot, Telegram user, chat, expected message, stable preview target, mode, stage, expiry, and idempotency key behind app-minted high-entropy tokens. Token consumption SHALL be transactional and require the expected flow stage and owner/chat/message binding; expired, replayed, malformed, stale-stage, or foreign tokens SHALL submit nothing. Callback data SHALL contain no raw URL, repository JSON, account identity, provider credential, or mutable policy state.

#### Scenario: A foreign forwarded button cannot act

- **WHEN** another Telegram user presents an unexpired confirmation token created for the owner
- **THEN** the callback is answered with a minimal safe refusal and GitHub receives no action request

#### Scenario: Concurrent replay has one winner

- **WHEN** two workers attempt to consume the same confirmation token concurrently
- **THEN** exactly one transition submits the action and the other observes the token as already consumed

#### Scenario: Expired confirmation cannot write

- **WHEN** the owner presses a confirmation token after its expiry
- **THEN** Telegram answers it as expired and submits no action

### Requirement: GitHub component results render without inference

The result message SHALL list metadata, provider star, and desired backup separately using the meaning returned by GitHub. `accepted` desired backup SHALL be rendered as desired-policy acceptance, never completed or verified backup; failed, refused, already-applied, and skipped outcomes SHALL remain distinguishable. A partial result SHALL name every successful and unsuccessful component and SHALL NOT offer or perform compensating provider mutation.

#### Scenario: Provider star succeeds and backup fails

- **WHEN** GitHub returns metadata succeeded, provider star succeeded, and desired backup failed
- **THEN** Telegram renders a partial result naming both successes and the backup failure without saying the whole action failed or succeeded atomically

#### Scenario: Missing optional metadata stays absent

- **WHEN** a preview or result has no description or primary language
- **THEN** Telegram omits those fields rather than inventing placeholder content

### Requirement: GitHub outages fail honestly

Preview or action network, timeout, authentication, invalid-response, and server failures SHALL use bounded retry only for transient classes and SHALL settle the update/flow with a safe actionable message. An uncertain action response SHALL retry only with the same idempotency key; Telegram SHALL never replace missing GitHub truth with a success message.

#### Scenario: Preview service is unavailable

- **WHEN** the Platform/GitHub preview route remains unreachable through the retry bound
- **THEN** Telegram reports that repository preview is unavailable and renders no action buttons

#### Scenario: Action response is lost

- **WHEN** the confirmed action request may have reached GitHub but the response is lost
- **THEN** Telegram retries only the same confirmed request identity or reports the outcome unknown, never submits a new provider mutation identity and never claims success
