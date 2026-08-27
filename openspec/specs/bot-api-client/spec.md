# bot-api-client Specification

## Purpose
The typed boundary through which this service calls the Telegram Bot API, and how it is tested without ever contacting Telegram.

## Requirements

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

### Requirement: Messages can be sent and edited as Telegram HTML with inline keyboards

The send and edit methods SHALL accept an HTML parse mode and an optional inline keyboard markup alongside the text, passing both through the typed client so rendered messages keep bold leads, hyperlinks, and buttons on the wire. Omitting markup SHALL leave those fields out of the request entirely. Failures SHALL surface through the existing error taxonomy unchanged, and the token SHALL appear nowhere in any error rendering or recorded request outside its documented path position.

#### Scenario: sendMessage carries parse mode and buttons

- **WHEN** `send_message` is called with HTML text and an inline keyboard against the harness
- **THEN** the JSON body carries `parse_mode` set for HTML and the identical button layout under `reply_markup`

#### Scenario: editMessageText carries them identically

- **WHEN** `edit_message_text` is called with HTML text and an inline keyboard against the harness
- **THEN** the JSON body carries the parse mode, the markup, and the target chat and message ids

#### Scenario: A plain message omits the fields

- **WHEN** either method is called with only text
- **THEN** the JSON body carries no `parse_mode` and no `reply_markup`
