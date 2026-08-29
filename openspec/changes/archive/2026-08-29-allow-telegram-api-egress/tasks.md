## 1. Reproduce the blocked egress

- [x] 1.1 RED: add `systemd_units_preserve_required_https_egress` to `services/webhook/tests/deployment_profile.rs`, asserting for both unit fixtures that `IPAddressDeny=any` is absent while `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK` and the existing role listener boundaries remain present; run `build-gate -- cargo test -p ratatoskr-telegram-webhook --test deployment_profile --locked systemd_units_preserve_required_https_egress -- --exact` and confirm the assertion fails by naming the webhook unit's public-egress denial.

## 2. Restore runtime dependency reachability

- [x] 2.1 GREEN: remove the bidirectional `IPAddressDeny`/`IPAddressAllow` policy from both systemd units without changing ports, binds, users, credentials, resource limits, address-family restrictions, or other hardening; rerun the exact test from 1.1 and observe it pass.
- [x] 2.2 Run `build-gate -- cargo test -p ratatoskr-telegram-webhook --test deployment_profile --locked` and observe all structural deployment tests pass, including the pre-existing port, role, schema-ordering, resource, and hardening checks.

## 3. Document the security boundary

- [x] 3.1 Update `deploy/README.md` to state that the checked-in units permit required outbound HTTPS and that application bind addresses, cloudflared, and the target-host firewall remain authoritative for ingress; documentation cannot start from a failing behavior test, so verify the documented ports and boundaries match both unit/environment fixtures with `rg` inspection.

## 4. Validate the complete change

- [x] 4.1 Run the non-mutating deployment runbook checks (`bash -n deploy/bin/telegram-ops deploy/tests/telegram_ops_test.sh` and `bash deploy/tests/telegram_ops_test.sh`) and observe PASS without contacting Telegram or changing the host.
- [x] 4.2 Run the full fenced gate from `DEVELOPMENT.md` through one top-level `build-gate`, then run `openspec validate allow-telegram-api-egress --strict`, `openspec validate --all --strict`, and `openspec validate --archived`; observe every check pass and review the final diff for unrelated changes, leaked credentials, weakened listener boundaries, or unsupported live-deployment claims.
