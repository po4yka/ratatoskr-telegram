## ADDED Requirements

### Requirement: Dialogue submission authority names its durable update

The current schema SHALL bind every repository dialogue in `submitting` state to the bot and Telegram update identity whose accepted payload released the action. That authority SHALL remain a reference to service-owned update state and SHALL contain no callback token or provider credential.

#### Scenario: Fresh schema enforces submission authority
- **WHEN** a repository dialogue transitions into `submitting`
- **THEN** its releasing update identity is stored in the same transaction and another update cannot replace it
