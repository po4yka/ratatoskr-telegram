# notification-delivery Specification

## Purpose
Defines how Telegram turns a canonical raised-notification fact into a durable, preference-filtered, privacy-minimized outbound message for an authorized chat.

## Requirements

### Requirement: Users can configure notification policy from an authorized private chat

An authorized user SHALL be able to inspect and change the notification policy for the current verified private chat through deterministic `/settings`, `/settings notifications on|off`, `/settings notification <class> on|off|inherit`, `/settings quiet-hours inherit|disabled|HH:MM-HH:MM`, and `/settings high-priority-bypass on|off` forms that re-check the actor and chat on every update. Policy SHALL include a global enabled switch, optional overrides for each known notification class, a quiet-hours mode of `inherit`, `disabled`, or `custom`, custom UTC start and end offsets when applicable, and whether high-priority notifications may bypass quiet hours.

#### Scenario: User disables one known class

- **WHEN** an authorized user disables `backup_outcome` in `/settings`
- **THEN** the persisted policy for that verified private chat disables that class without changing sibling class choices

#### Scenario: Foreign actor attempts to change preferences

- **WHEN** another user or chat attempts to change a policy it does not own
- **THEN** the transition is refused and the stored policy is unchanged

#### Scenario: Custom quiet hours are malformed

- **WHEN** a user submits an equal-bound, out-of-range, or otherwise malformed custom quiet-hours window
- **THEN** the dialogue remains recoverable, explains the safe validation failure, and stores no partial policy

### Requirement: Notification admission is typed, durable, and deduplicated

The dispatcher SHALL consume only a canonical `platform.notification.raised.v1` envelope from `evt.platform.notification.raised.v1`. It SHALL validate the typed contract, recipient linkage, chat authorization, and notification identity before producing a decision, and SHALL persist one terminal or deferred decision per notification and target chat so bus redelivery cannot enqueue a duplicate.

Telegram SHALL resolve eligible chats through an explicit admitted private-chat-to-identity binding and SHALL NOT infer a destination because a chat identifier resembles a Telegram user identifier.

#### Scenario: Same notification is re-raised under a new event id

- **WHEN** two valid envelopes carry one notification identity to the same recipient and chat
- **THEN** exactly one notification decision and at most one outbound job exist for that chat

#### Scenario: Recipient has no eligible private chat

- **WHEN** a valid notification targets a user with no active authorized private chat
- **THEN** Telegram records a safe undeliverable outcome, sends nothing, and acknowledges the fact without retrying forever

#### Scenario: Similar numeric chat is not bound

- **WHEN** a known chat identifier numerically resembles the recipient's Telegram user identifier but has no explicit admitted binding
- **THEN** Telegram does not select that chat or send a Bot API request

#### Scenario: Payload or recipient binding is invalid

- **WHEN** the envelope is malformed, names another event type, or targets a user not linked to the eligible chat
- **THEN** Telegram creates no outbound job and records only a bounded failure class without payload content

### Requirement: User policy controls suppression and quiet hours

Telegram SHALL apply the target chat's global switch and class override before enqueueing delivery. A custom user quiet-hours window SHALL take precedence over a producer hint; `disabled` SHALL ignore the hint; `inherit` SHALL use a valid producer hint when present. A notification inside the effective window SHALL be deferred until its end unless it is high priority and the user's own policy explicitly permits bypass.

#### Scenario: Class override suppresses delivery

- **WHEN** the notification class is disabled for the target chat
- **THEN** the decision is terminal `suppressed`, no outbound job is created, and no Bot API call occurs

#### Scenario: Normal notification arrives during wrap-around quiet hours

- **WHEN** a normal-priority notification arrives inside a configured window whose start is later than its end
- **THEN** one durable decision is deferred until the next window end and no early Bot API call occurs

#### Scenario: User permits high-priority bypass

- **WHEN** a high-priority notification arrives during quiet hours and the user enabled high-priority bypass
- **THEN** Telegram enqueues it immediately

#### Scenario: Unknown well-formed class arrives

- **WHEN** a later producer sends a well-formed class this build does not recognize
- **THEN** Telegram preserves the class token, applies the global preference without inventing a per-class default, and records the decision safely

### Requirement: Notification rendering is minimal and safe

The outbound message SHALL carry only the contract title, optional bounded detail, and authorized opaque links derived from correlation references. Dynamic text SHALL be escaped for Telegram markup; raw domain payloads, credentials, provider errors, usernames, URLs with secrets, and diagnostic traces SHALL NOT be rendered or logged.

#### Scenario: Notification text contains markup metacharacters

- **WHEN** a valid title or detail contains Telegram markup metacharacters
- **THEN** the delivered message displays them as text and cannot add tags, links, or buttons

### Requirement: Notification delivery reuses the durable sender

An admitted notification SHALL enter the existing durable outbound queue and inherit per-chat ordering, global and per-chat rate limiting, retry bounds, and distinct permanent-failure classification. Direct interaction responses SHALL retain priority over background notifications, and a provider acknowledgment SHALL be required before delivery is recorded complete.

#### Scenario: Interactive response competes with notification

- **WHEN** a direct command response and a ready background notification target the same chat
- **THEN** the direct response is selected first while both remain durably ordered

#### Scenario: Bot API permanently refuses the chat

- **WHEN** Telegram reports the bot blocked or the chat forbidden for a notification send
- **THEN** the notification reaches a terminal safe failure without unbounded retry
