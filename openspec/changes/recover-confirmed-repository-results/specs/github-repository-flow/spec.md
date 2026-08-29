## ADDED Requirements

### Requirement: Confirmed actions recover one durable result projection

After a valid confirmation releases a repository action, Telegram SHALL retain enough durable authority to resume only that action until dialogue completion and its result message are committed atomically. Recovery SHALL reuse the dialogue's stable idempotency key and confirmation evidence, SHALL NOT mint a new provider mutation identity, and SHALL NOT let another update or actor inherit the consumed confirmation.

#### Scenario: Result enqueue fails after action success
- **WHEN** Platform accepts a confirmed action but local storage fails before dialogue completion and result enqueue commit
- **THEN** the original durable update remains recoverable and later commits one completion and one result job using the same action identity

#### Scenario: Another callback cannot resume submitting work
- **WHEN** a different Telegram update presents the already-consumed confirmation while its dialogue is submitting
- **THEN** it receives the common replay refusal and releases no action or result projection
