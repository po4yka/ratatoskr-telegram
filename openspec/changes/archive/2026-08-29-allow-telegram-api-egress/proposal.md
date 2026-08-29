## Why

The shipped systemd units deny every public IP address while both runtime roles call the configured Telegram Bot API during startup and normal operation. With the production default of `https://api.telegram.org`, the hardened profile therefore prevents the service from starting or delivering messages.

## What Changes

- Remove the unit-level IP address allowlist that blocks hostname-based public HTTPS dependencies from both Telegram runtime roles.
- Keep listener exposure bounded by the existing bind addresses, trusted cloudflared path, host firewall contract, restricted address families, and other systemd hardening directives.
- Extend structural deployment validation so both units must permit the configured Bot API egress path and a regression to `IPAddressDeny=any` without public egress fails.
- Document that target-host ingress policy remains an operator-owned firewall responsibility and that repository validation does not claim a live Telegram connection.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `deployment-profile`: Require the production-shaped units to preserve outbound reachability to configured HTTPS dependencies while retaining the existing listener and host-firewall boundaries.

## Impact

- Affected surfaces: both systemd units, their structural Rust deployment test, and the single-host deployment runbook.
- No API, event contract, schema, credential format, runtime Rust implementation, dependency, port allocation, webhook registration, or live host state changes.
- The change broadens unit-level network reachability because systemd's address allowlist is not direction-specific; inbound exposure continues to be controlled by application bind addresses, cloudflared routing, and the host firewall described by the deployment contract.
