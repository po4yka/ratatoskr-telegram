## ADDED Requirements

### Requirement: Released confirmed work retains its source update until projection

After a confirmed action has been released, the worker SHALL keep its source update processable through transient external uncertainty and local completion or outbound-storage failure. The update SHALL become terminal only when an explicit refusal is safely rendered or dialogue completion and the result projection have committed.

#### Scenario: Confirmed result storage recovers
- **WHEN** the result projection transaction fails after the action may have succeeded
- **THEN** the update retains its payload and a later claim retries under the original confirmation and idempotency identity
