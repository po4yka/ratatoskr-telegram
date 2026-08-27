## MODIFIED Requirements

### Requirement: Bot API calls go through one typed client

Bot API calls SHALL be made through a single client type exposing typed methods — `get_me`, `set_webhook`, `send_message`, `edit_message_text`, `answer_callback_query`, `send_chat_action`, `get_file`, and a bounded file download that streams the resolved file's bytes — over the pinned `teloxide` dependency. The client SHALL target the configured base URL with a configured timeout, and failures SHALL surface as one error taxonomy of `network`, `rate_limited` (carrying the retry delay), `api` (carrying Telegram's description), `json` and local-file-transfer classes. The token-carrying download URL SHALL exist only inside the client boundary and appear in no error rendering, log line, or stored value.

#### Scenario: get_me returns the bot identity from the configured endpoint

- **WHEN** `get_me` is called against a harness server serving a recorded `getMe` response
- **THEN** the call resolves to the bot's user id and username from that response

#### Scenario: A Telegram API error surfaces as its class without the token

- **WHEN** the harness answers an API call with a Bot API error body
- **THEN** the call fails with the `api` class carrying the description, and the bot token appears nowhere in the error rendering

#### Scenario: A rate-limited answer carries its retry delay

- **WHEN** the harness answers 429 with `retry_after`
- **THEN** the call fails with the `rate_limited` class carrying that delay

#### Scenario: An unreachable endpoint is a network failure

- **WHEN** the client targets a closed port
- **THEN** the call fails with the `network` class

#### Scenario: A file download streams the served bytes without loading them whole

- **WHEN** a bounded download runs against a harness serving a resolved file path
- **THEN** the returned stream yields exactly the served bytes, and no download method returns before its consumer finishes reading or abandons it
