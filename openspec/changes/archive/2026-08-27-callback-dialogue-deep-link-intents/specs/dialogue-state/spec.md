## Purpose

Defines durable, restart-safe finite-state interactions whose awaiting-input steps expire and whose transitions admit only one expected version and actor scope.

## ADDED Requirements

### Requirement: Awaiting-input dialogue state is durable and minimal

The service SHALL persist each dialogue with an app-minted identifier, kind, bot, owning Telegram user, chat, current step, monotonic version, bounded typed payload or resource references, optional expected message, expiry, lifecycle state, and timestamps. Dialogue records SHALL contain no provider credential, Platform session credential, raw private message history, or domain-owned body.

#### Scenario: Dialogue survives a process restart

- **WHEN** an awaiting-input dialogue is read through a newly created database handle before its expiry
- **THEN** the same owner scope, step, version, safe payload references, and expiry are available for the next transition

### Requirement: Transitions require expected scope, step, and version

An input transition SHALL advance a live dialogue only when bot, Telegram user, chat, expected step, and expected version all match. The accepted transition SHALL increment the version atomically; duplicate, foreign, and stale transitions SHALL change no state and SHALL release no action.

#### Scenario: Only one writer advances an awaiting dialogue

- **WHEN** two workers concurrently submit the same valid transition for one dialogue version
- **THEN** one advances the dialogue and increments its version while the other receives a stale-state refusal

### Requirement: Dialogue timeout is an explicit terminal transition

At or after a dialogue's expiry, a transition attempt or cleanup pass SHALL atomically move an active or awaiting-input dialogue to `expired` without executing its pending action. Completed and cancelled dialogues SHALL remain terminal, and no later input SHALL revive any terminal dialogue.

#### Scenario: Awaiting input times out

- **WHEN** a transition is attempted at the dialogue expiry boundary
- **THEN** the dialogue reads `expired`, its version advances once, and the pending action is not released

### Requirement: Dialogue cleanup is bounded and preserves domain work

The cleanup pass SHALL expire stale active dialogues in bounded batches and SHALL remove only terminal dialogue rows whose retention window has elapsed. Expiry, cancellation, or deletion of dialogue state SHALL not cancel or delete a domain operation already submitted outside the dialogue.

#### Scenario: Cleanup expires stale state but preserves an operation reference

- **WHEN** cleanup expires an awaiting dialogue that references an already-created operation
- **THEN** the dialogue becomes expired and the operation and its message binding remain unchanged
