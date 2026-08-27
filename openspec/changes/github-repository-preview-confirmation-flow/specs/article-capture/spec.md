## MODIFIED Requirements

### Requirement: Authorized private messages parse into capture intents

A processable message update from an authorized sender in a private chat SHALL be parsed into typed intents: a message whose text is a single http(s) URL SHALL become a capture intent except when it is a canonical GitHub repository URL handled by the GitHub repository preview flow; a `/summarize <url>` command with a well-formed URL argument SHALL become a capture intent even when its argument is a GitHub repository URL, because the explicit command names article/content capture; and any other plain text - including a `/summarize` without a usable URL argument - SHALL leave the update settling as unsupported with class-only telemetry and no outbound traffic. A forwarded message whose text or caption contains an http(s) link SHALL become that same capture intent with its forward origin preserved as bounded metadata on the intent record and carried with the submission; when several links are present the first one is captured. A forwarded message carrying no link and no supported attachment settles unsupported.

#### Scenario: A bare URL becomes a capture intent

- **WHEN** an enabled sender in a private chat sends a message whose text is `https://example.test/article`
- **THEN** the update settles processed and one capture submission for that URL is attempted

#### Scenario: A canonical GitHub repository URL routes to preview

- **WHEN** an enabled sender sends `https://github.com/owner/repository`
- **THEN** the GitHub repository preview flow handles it and no content capture submission is attempted

#### Scenario: The summarize command form parses identically

- **WHEN** an enabled sender sends `/summarize https://example.test/article`
- **THEN** the update settles processed and the capture submission derives from the same intent kind and URL

#### Scenario: Summarize explicitly captures a GitHub URL as content

- **WHEN** an enabled sender sends `/summarize https://github.com/owner/repository`
- **THEN** the explicit capture command submits that URL through the article flow rather than opening repository preview

#### Scenario: Text without a usable URL is unsupported

- **WHEN** an enabled sender sends `hello world`, or `/summarize` with no argument, or `/summarize ftp://example.test/x`
- **THEN** the update settles unsupported, nothing is submitted to Platform, and no message is sent

#### Scenario: A forwarded channel post with a link captures with provenance

- **WHEN** an enabled sender forwards a channel post whose text contains `https://example.test/story`
- **THEN** the capture submission for that URL proceeds and the intent record persists the forward origin facts (origin kind, origin identifiers, original date)

#### Scenario: The first of several forwarded links is captured

- **WHEN** a forwarded message's caption contains two http(s) links
- **THEN** exactly one capture submission results, referencing the first link in message order

#### Scenario: A forward with no link and no attachment is unsupported

- **WHEN** an enabled sender forwards a plain-text note containing no URL
- **THEN** the update settles unsupported with no submission and no outbound reply
