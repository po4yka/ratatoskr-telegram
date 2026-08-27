# Telegram integration interfaces

## Bot API inbound

Webhook updates for messages, commands, documents, forwarded messages, callback queries, and supported Mini App data. Validate secret/header, body limits/schema, bot identity, and deduplicate update ID.

## Bot API outbound

Send/edit/delete/answer-callback/file operations through a dispatcher with per-chat/global ordering, rate limits, retry-after, idempotency, and safe rendering/escaping.

## Platform/domain

Identity assertion exchange; operation creation/status; article capture; and typed GitHub repository preview/action calls through Platform's authenticated `/v1/gh/repositories` gateway. Repository preview is read-only. Every `metadata`, `track`, or `star` action carries server-side confirmation evidence and one stable idempotency identity. GitHub returns its aggregate plus metadata/provider-star/desired-backup component facts; Telegram renders those facts without compensation or local success inference. Star-list selection remains outside this surface.

## Mini App

Verify raw `initData` server-side, enforce auth-age/replay/bot audience/user binding, issue short-lived signed assertion for Platform, and consume opaque `startapp` intent. `web_app_data` and client fields remain untrusted.

Errors distinguish unauthorized, expired/replayed callback/intent, invalid update/file, unsupported capability, provider partial, Telegram rate/API, and transient dependency states.
