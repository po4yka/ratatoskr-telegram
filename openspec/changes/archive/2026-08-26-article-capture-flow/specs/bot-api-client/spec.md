## ADDED Requirements

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
