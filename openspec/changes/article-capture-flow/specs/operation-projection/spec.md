## ADDED Requirements

### Requirement: Terminal renders compose links from server-side state

A terminal render for a bound operation MAY compose, beside the status-led escaped body, a fallback hyperlink to the operation's source address and one URL button whose target is the Mini App deep link carrying an opaque intent identifier resolved from this service's own intent records. Non-terminal renders SHALL remain plain status-led bodies without buttons or hyperlinks beyond those the escaping renderer already permits.

#### Scenario: Buttons ride only on the terminal render

- **WHEN** an operation's binding receives a running frame followed by its succeeded terminal
- **THEN** the running edit job carries no reply markup while the single terminal job carries the deep-link button and fallback hyperlink

#### Scenario: Markup-only terminal changes still edit

- **WHEN** a terminal render's composed body equals the previous render but adds markup
- **THEN** the job is not suppressed as identical content, because the payload hash covers markup too
