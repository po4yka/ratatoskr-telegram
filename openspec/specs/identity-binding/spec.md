# identity-binding Specification

## Purpose
TBD - created by archiving change xr-021-telegram-pdf-capture-and-identity. Update Purpose after archive.

## Requirements

### Requirement: A session exchange binds the Telegram identity to the Platform user

After every successful network exchange of a Telegram assertion for a Platform session, the process SHALL write the returned Platform user id to `telegram.identities.internal_user_id` for the exchanged Telegram user. Platform is authoritative: a changed mapping overwrites the stored one, an unchanged mapping changes nothing, and a Telegram user without an identity row updates nothing and is not an error. The write SHALL NOT run on a cache hit and SHALL NOT be able to fail the exchange. A failed write SHALL be logged by safe class and counted, and the binding SHALL be retried at the next network exchange. Both runtime roles SHALL install the same binding.

#### Scenario: A first capture binds the sender

- **WHEN** an enabled sender's first capture triggers a session exchange and Platform returns an internal user id
- **THEN** the sender's identity row carries that id without any other write

#### Scenario: The binding is idempotent and Platform wins

- **WHEN** the same mapping is bound twice, and then Platform returns a different user id for the same Telegram user
- **THEN** the second bind of the same id changes nothing and the changed id overwrites the stored one

#### Scenario: A cache hit does not bind

- **WHEN** a second operation reuses the cached session
- **THEN** no further binding write is attempted

#### Scenario: A binding failure does not fail the exchange

- **WHEN** the binding write errors
- **THEN** the session is still returned to the caller and the failure is counted with outcome `failed`

#### Scenario: A bound user receives a notification

- **WHEN** a sender has been bound by a session exchange and then bound to a private chat, and a notification for that internal user is processed
- **THEN** the notification is enqueued for that chat instead of ending with no eligible chat
