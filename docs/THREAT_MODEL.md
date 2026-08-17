# Telegram integration threat model

## Assets

Bot/webhook secrets, identity bindings, private messages/files/URLs, callback/deep-link intent, Mini App session bridge, provider-write confirmation, notification privacy, and bot availability.

## Threats and controls

- **Forged webhook/update:** secret token, TLS/reverse-proxy policy, schema/body limits, update-ID dedupe.
- **Identity confusion/group impersonation:** exact Telegram user/chat binding and authorization per action.
- **Callback/deep-link replay/tampering:** opaque random tokens, server-side payload, expiry, scope, single use.
- **Mini App forgery/replay:** verify raw `initData`, signature, auth time, audience, nonce/replay, user binding.
- **Malicious file/URL/forward:** type/size/count limits, safe download, no execution, delegate extraction/import.
- **Provider-write surprise:** explicit confirmation, capability/scope check, audit, truthful component result.
- **Message injection/privacy leak:** escape rendering, safe previews, destination authorization, no sensitive logs.
- **Rate-limit/order failure:** durable dispatcher, per-chat serialization, retry-after, stale projection rejection.
- **Bot token theft:** secret manager, rotation, no DB/log/event, restricted deployment identity.

Re-review for groups/admin actions, payments, inline mode, userbot/MTProto, arbitrary file types, or public bot onboarding.
