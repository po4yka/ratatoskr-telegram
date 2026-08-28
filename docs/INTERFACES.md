# Telegram integration interfaces

## Bot API inbound

Webhook updates for messages, commands, documents, forwarded messages, callback queries, and supported Mini App data. Validate secret/header, body limits/schema, bot identity, and deduplicate update ID.

Callback data is exactly one opaque registry token. Presentation verifies bot/user/chat/message,
expiry, one-time state, and expected dialogue version transactionally. An unusable recognized
callback is answered promptly with the same expired-state message and never releases another
action. Exact `/start <64-character-token>` messages resolve the owner-scoped `operation_status`
intent once; no URL, operation ID, or business payload is parsed from the command.
Exact `/search <query>` (1–256 Unicode scalar values), `/unread`, and `/read <64-character-token>`
forms are accepted only after the existing private-chat authorization gate. Invalid forms are
answered locally and make no Platform library request.

## Bot API outbound

Send/edit/delete/answer-callback/file operations through a dispatcher with per-chat/global ordering, rate limits, retry-after, idempotency, and safe rendering/escaping.

## Platform/domain

Identity assertion exchange; operation creation/status; article capture; typed library
`GET /v1/library/search` and idempotent `PUT /v1/library/items/{analysis_id}/read-state`; and typed
GitHub repository preview/action calls through Platform's authenticated `/v1/gh/repositories`
gateway. Telegram checks `library.search` before querying and independently checks
`library.read_state` before issuing or consuming read actions. It renders only the five minimized
summaries returned by Platform, escapes bounded HTML fields, and treats Platform's returned read
resource as authority. Repository preview is read-only. Every `metadata`, `track`, or `star` action carries server-side confirmation evidence and one stable idempotency identity. GitHub returns its aggregate plus metadata/provider-star/desired-backup component facts; Telegram renders those facts without compensation or local success inference. Star-list selection remains outside this surface.

## Mini App

Planned in item 9: verify raw `initData` server-side, enforce auth-age/replay/bot audience/user
binding, issue a short-lived signed assertion for Platform, and bind Mini App launch state to opaque
server authority. `web_app_data` and client fields remain untrusted. Item 8's implemented Bot API
deep-link transport is `/start <token>`, not Mini App authentication.

Errors distinguish unauthorized, expired/replayed callback/intent, invalid update/file, unsupported capability, provider partial, Telegram rate/API, and transient dependency states.
