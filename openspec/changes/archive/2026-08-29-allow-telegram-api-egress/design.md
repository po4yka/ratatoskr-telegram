## Context

See `proposal.md` for the failure. Both roles use a configurable Bot API base URL and contact it during startup. The same processes also need PostgreSQL, Platform, and NATS paths. The current `IPAddressDeny=any` plus a private-range allowlist applies to both inbound and outbound traffic, so it cannot preserve the current inbound restriction while admitting a hostname-based public HTTPS dependency whose resolved addresses are outside that fixed list.

The existing deployment boundary already assigns listener exposure separately: the webhook public listener binds loopback behind cloudflared, operator listeners use fixed ports governed by the host firewall, and the dispatcher has no public listener. Repository validation is structural and must not contact Telegram or mutate a host.

## Goals / Non-Goals

**Goals:**

- Make both shipped units compatible with the configured Telegram Bot API and required HTTPS dependencies.
- Retain every independently effective process, filesystem, resource, address-family, listener, credential, and supervision control.
- Add a deterministic regression test that fails on the current public-egress denial without making a live network request.

**Non-Goals:**

- Pin Telegram provider IP ranges or assume that a DNS answer is stable.
- Change application listeners, ports, firewall rules, cloudflared routes, credentials, or runtime Rust behavior.
- Install or restart units, register a webhook, or claim live Telegram connectivity.

## Decisions

### Remove the bidirectional address allowlist from both units

Delete `IPAddressDeny=any` and its private-range `IPAddressAllow` companion from both unit files. Keep `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK` and all other hardening.

Hard-coding current Telegram address ranges was rejected because the Bot API endpoint is hostname-based, its addresses are provider-controlled, and the service supports a configured Bot API origin. Adding an allow-all address override was rejected because it is equivalent in reachability while preserving misleading policy text. A directional egress policy belongs in the target firewall or a deployment-specific network policy that can distinguish flows and resolve current destinations.

### Keep ingress authority in the existing deployment layers

The unit change does not alter bind configuration: public webhook traffic still reaches loopback `8182` only through cloudflared, operator ports remain under the target-host firewall, and dispatcher remains operator-only. The runbook will state explicitly that removing the bidirectional unit filter makes installation contingent on those existing host controls.

### Prove the behavior structurally

Add one Rust regression test before editing the units. It will read both checked-in units and assert that neither ships the known public-egress-denying policy, while also asserting that the existing listener values and restricted address families remain present. The predicted RED is an assertion failure naming `IPAddressDeny=any` in the webhook unit; GREEN requires correcting both units. No test contacts Telegram, resolves DNS, or needs a production token.

## Risks / Trade-offs

- [A compromised process can initiate connections to more addresses than under the current unit filter] → retain least privilege and syscall/address-family restrictions, keep secrets narrowly readable, and use the target firewall for directional egress policy when required.
- [Removing the filter could be mistaken for publishing operator listeners] → preserve exact bind/port assertions and document the independent host-firewall/cloudflared boundary.
- [Structural validation cannot prove live DNS, TLS, routing, or Telegram acceptance] → report only artifact-level evidence; live verification remains an explicit authorized deployment action.

## Migration Plan

1. Validate the corrected artifacts and full repository gate without host mutation.
2. In a separately authorized deployment, install both unit files together, run `systemd-analyze verify`, reload systemd, and restart the two roles.
3. Confirm readiness and provider reachability through safe operational telemetry without logging credentials or private content.
4. If rollback is required, restore the previous units only with an operator override that preserves required egress; an unmodified rollback recreates the known outage.
