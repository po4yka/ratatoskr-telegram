## Purpose

The typed boundary through which this service calls the Telegram Bot API, and how it is tested without ever contacting Telegram.

## ADDED Requirements

### Requirement: Bot API calls go through one typed client

Bot API calls SHALL be made through a single client type exposing typed methods — `get_me`, `set_webhook`, `send_message`, `edit_message_text`, `answer_callback_query` and `send_chat_action` — over the pinned `teloxide` dependency. The client SHALL target the configured base URL with a configured timeout, and failures SHALL surface as one error taxonomy of `network`, `rate_limited` (carrying the retry delay), `api` (carrying Telegram's description), `json` and local-file-transfer classes.

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

### Requirement: The client is exercised only against local harnesses in tests

Tests SHALL point the client at a local server answering recorded fixtures, and SHALL NOT contact api.telegram.org. Request assertions SHALL verify method paths carry no secret outside the URL path contract and payload bodies match the typed inputs.

#### Scenario: sendMessage posts its typed payload to the harness

- **WHEN** `send_message` is called against the harness
- **THEN** the request path addresses the configured bot, and the JSON body carries the chat id and text given

### Requirement: Update payloads parse against the Bot API schema with recorded fixtures

Update bodies SHALL deserialize into the shared update type where the envelope (an `update_id` and valid JSON) is well-formed; an unknown kind inside a well-formed envelope SHALL be preserved as unsupported rather than rejected; anything else SHALL fail parsing.

#### Scenario: A recorded message update parses with its id and kind

- **WHEN** a synthetic message-update fixture is parsed
- **THEN** the update id and message kind are available typed

#### Scenario: An unknown kind is unsupported, not malformed

- **WHEN** a fixture carries an update kind key this build does not know
- **THEN** parsing succeeds and the value reports itself as an unrecognized kind

#### Scenario: An envelope without an update id fails

- **WHEN** a JSON body has no `update_id`
- **THEN** parsing fails
