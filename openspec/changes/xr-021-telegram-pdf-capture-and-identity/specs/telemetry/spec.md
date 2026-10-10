## ADDED Requirements

### Requirement: Identity binding outcomes are counted

The metric registry SHALL own `telegram_identity_binding_total` with a closed `outcome` label of `bound`, `unchanged` and `failed`, incremented once per post-exchange binding attempt. No label value SHALL derive from a Telegram or Platform identifier.

#### Scenario: Binding outcomes appear in the exposition

- **WHEN** a binding is written, repeated and then fails
- **THEN** the exposition counts one `bound`, one `unchanged` and one `failed` sample
