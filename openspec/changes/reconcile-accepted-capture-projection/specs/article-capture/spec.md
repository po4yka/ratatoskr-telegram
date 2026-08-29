## MODIFIED Requirements

### Requirement: An accepted capture acknowledges with one bound message

After Platform accepts a capture, the service SHALL atomically persist the operation's message binding, its owner-bound opaque intent, and exactly one acknowledgment send job. A local failure before that transaction commits SHALL leave no partial projection and SHALL keep the source update recoverable; retry after restart SHALL reuse the original Platform idempotency key and converge on the accepted operation. A repeated submission that resolves to an operation already bound live in the same chat SHALL NOT enqueue a second acknowledgment.

#### Scenario: Early progress is not lost to an unsent ack
- **WHEN** Platform's first progress frame for an operation arrives while its acknowledgment send is still queued
- **THEN** the frame is applied to the binding and renders once the message exists, rather than counting as unbound traffic

#### Scenario: Double send produces one tracked message
- **WHEN** the same sender submits the same URL twice and both resolve to one operation
- **THEN** the chat holds exactly one acknowledged message bound to that operation

#### Scenario: Accepted capture survives local projection failure
- **WHEN** Platform accepts a capture but the atomic local projection cannot commit
- **THEN** no partial binding, intent, or acknowledgment remains and recovery reuses the same submission identity until all three commit once
