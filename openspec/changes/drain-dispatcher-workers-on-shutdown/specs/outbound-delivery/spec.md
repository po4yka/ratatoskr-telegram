## ADDED Requirements

### Requirement: Outbound delivery participates in graceful drain

After dispatcher shutdown cancellation, outbound workers SHALL claim no new jobs. A job whose Bot API request already started SHALL retain the bounded opportunity to reach the delivery settlement required by its known or ambiguous provider outcome before the worker exits.

#### Scenario: Cancellation arrives between jobs
- **WHEN** an outbound worker settles one job after shutdown has been signalled while another job is ready
- **THEN** it exits without claiming the next job
