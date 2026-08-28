## ADDED Requirements

### Requirement: Background notifications have explicit lower queue priority

Outbound jobs SHALL distinguish direct interaction responses from background notifications. Ready direct responses SHALL be selected before background notifications for the same chat without bypassing per-chat ordering, provider rate limits, or durability, and background work SHALL remain bounded so it cannot starve indefinitely.

#### Scenario: Direct response and notification are both ready

- **WHEN** one direct response and one background notification are ready for the same chat
- **THEN** the direct response is attempted first and the notification remains durably eligible

### Requirement: Deferred notifications become ready without duplication

A notification deferred by quiet hours SHALL not be claimable before its recorded release time. At or after that time it SHALL enter normal outbound ordering exactly once, and repeated scheduler/consumer wakeups SHALL NOT create another job.

#### Scenario: Quiet-hours boundary passes

- **WHEN** the stored release time arrives for a deferred notification
- **THEN** one existing job becomes claimable and no second outbound payload is inserted
