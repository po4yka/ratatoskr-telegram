## Purpose

Restrict interaction to known, enabled Telegram principals on private chats. This capability defines which principals may reach domain processing, how the first principal is bootstrapped from deployment configuration, and how denials behave without revealing enrollment status.

## ADDED Requirements

### Requirement: Updates are authorized before any domain action

The webhook worker SHALL resolve the sender identity record and the chat record for every processable update before dispatching it to domain processing, and SHALL evaluate an authorization policy against them. An update whose sender has no identity record, whose identity record is disabled, or which was received outside a private chat SHALL NOT reach domain processing and SHALL settle as `denied`.

#### Scenario: Unknown sender never reaches processing

- **WHEN** an update arrives from a Telegram user with no persisted identity record
- **THEN** the update settles as `denied`
- **AND** no outbound Bot API request is made for it

#### Scenario: A disabled identity is treated like an unknown one

- **WHEN** an update arrives from a sender whose identity record exists but is disabled
- **THEN** the update settles as `denied` with no outbound Bot API request
- **AND** the observable behaviour is identical to the unknown-sender denial

#### Scenario: Non-private chats are denied

- **WHEN** a processable message arrives in a group or supergroup chat from an enabled identity
- **THEN** the update settles as `denied`
- **AND** no chat record is created for that group

#### Scenario: The enabled owner in a private chat proceeds

- **WHEN** a processable update arrives from an enabled identity in a private chat
- **THEN** the authorization gate passes and normal processing continues

### Requirement: The bootstrap owner is provisioned from deployment configuration

The webhook role SHALL ensure an enabled identity record exists for the configured owner Telegram user id when it starts, inserting the record only when absent and leaving an existing record untouched. A deliberately disabled owner record SHALL survive restarts unchanged.

#### Scenario: A fresh database gains its owner at startup

- **WHEN** the webhook starts against a database holding no identity records and a valid owner Telegram user id is configured
- **THEN** exactly one enabled identity record for that id exists once startup completes

#### Scenario: Restart does not resurrect a disabled owner

- **WHEN** the owner identity record was disabled after provisioning and the webhook restarts with the same configured owner id
- **THEN** the record stays disabled and no additional record appears

### Requirement: Denial reveals nothing

Denied updates SHALL produce no reply and no outbound Bot API traffic. Denials of unknown senders, disabled identities, and non-private chats SHALL be indistinguishable to the sender. Telemetry MAY count denials by outcome class but MUST NOT include user identifiers, chat titles, or content in metric labels, log fields, or error reports.

#### Scenario: Denials are silent and uniform

- **WHEN** any update is denied by the authorization gate
- **THEN** the sender observes no reply and no delivery failure attributable to authorization
- **AND** unknown, disabled, and non-private denials are externally indistinguishable

#### Scenario: Telemetry carries no identifiers for denials

- **WHEN** a denial is recorded in logs or metrics
- **THEN** the record identifies the outcome class only and contains no Telegram user id, chat title, names, or message content
