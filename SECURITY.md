# Security Policy for Ratatoskr Telegram

Report vulnerabilities privately. Do not publish bot tokens, webhook secrets, raw `initData`, Telegram user/chat identifiers tied to a person, private forwarded messages/files, callback payloads, or production update bodies.

Security review is required for webhook setup, secret validation, bot token storage, identity binding, access allowlists, callback/deep-link tokens, Mini App authentication, file download, provider-write confirmations, notifications, message rendering, and logs.

Baseline: Bot API only; no hidden MTProto/userbot; verify webhook secret; deduplicate updates; validate `initData` signature/auth time/replay and exact bot audience; opaque expiring one-time intents; least-privilege chat access; escape/limit output; no GitHub/provider credentials; rate-limited ordered delivery; redacted telemetry.
