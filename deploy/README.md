# Single-host deployment

This profile follows `ratatoskr-workspace/docs/DEPLOYMENT_TARGET.md` and the workspace port
contract: the webhook listens on loopback `8182` behind the
trusted cloudflared path, while operator listeners use `9467` (webhook) and `9468` (dispatcher).
Never publish either operator port through ingress. The monitoring bridge and Tailscale ranges in
the units are examples that must be checked against the target before installation.

Install the binaries in `/usr/local/bin`, the two examples in `/etc/ratatoskr`, four root-owned
secret files under `/etc/ratatoskr/secrets`, the logrotate policy in `/etc/logrotate.d`, and the
units in `/etc/systemd/system`. Environment files are `root:<role-group> 0640`. The shared bot
token and Platform signing key are `root:ratatoskr-telegram-secrets 0640`; both service users are
members of that narrow group. The webhook secret is `root:ratatoskr-telegram-webhook 0640`, and the
NATS credentials are `root:ratatoskr-telegram-dispatcher 0640`. Directories remain root-owned and
not group-writable. Logs stay on NVMe under `/mnt/nvme/ratatoskr/logs`.

After creating the named service users/groups and provisioning PostgreSQL/NATS under the workspace
profile, an authorized operator installs the non-secret artifacts explicitly:

```console
$ sudo install -d -o root -g root -m 0755 /etc/ratatoskr /etc/ratatoskr/secrets
$ sudo install -d -o root -g root -m 0755 /mnt/nvme/ratatoskr/logs
$ sudo install -o root -g root -m 0755 target/release/ratatoskr-telegram-webhook /usr/local/bin/
$ sudo install -o root -g root -m 0755 target/release/ratatoskr-telegram-dispatcher /usr/local/bin/
$ sudo install -o root -g root -m 0644 deploy/systemd/*.service /etc/systemd/system/
$ sudo install -o root -g root -m 0644 deploy/logrotate/ratatoskr-telegram /etc/logrotate.d/
$ sudo install -o root -g ratatoskr-telegram-webhook -m 0640 deploy/systemd/webhook.conf.example /etc/ratatoskr/telegram-webhook.conf
$ sudo install -o root -g ratatoskr-telegram-dispatcher -m 0640 deploy/systemd/dispatcher.conf.example /etc/ratatoskr/telegram-dispatcher.conf
```

Replace every `CHANGE-ME`, provision the four secret files with the ownership described above,
and let Platform provision the fixed `ratatoskr_telegram_notifications` durable before enabling
the dispatcher. Validate both effective configurations and the fresh schema contract without
binding listeners:

```console
$ sudo -u ratatoskr-telegram-webhook /usr/local/bin/ratatoskr-telegram-webhook check-config
$ sudo -u ratatoskr-telegram-webhook /usr/local/bin/ratatoskr-telegram-webhook check-schema
$ sudo -u ratatoskr-telegram-dispatcher /usr/local/bin/ratatoskr-telegram-dispatcher check-config
$ sudo -u ratatoskr-telegram-dispatcher /usr/local/bin/ratatoskr-telegram-dispatcher check-schema
```

Starting services and changing firewall/ingress are separate approved writes. When authorized:

```console
$ sudo systemctl daemon-reload
$ sudo systemctl enable --now ratatoskr-telegram-webhook.service
$ sudo systemctl enable --now ratatoskr-telegram-dispatcher.service
$ curl --fail --silent http://127.0.0.1:9467/health/ready
$ curl --fail --silent http://127.0.0.1:9468/health/ready
```

Only `127.0.0.1:8182` enters the trusted cloudflared route. Host firewall rules must deny public
access to `9467` and `9468`; monitoring reaches them only on the explicitly trusted host network.

Validate without starting or changing the host:

```console
$ cargo test -p ratatoskr-telegram-webhook --test deployment_profile --locked
$ bash -n deploy/bin/telegram-ops deploy/tests/telegram_ops_test.sh
$ bash deploy/tests/telegram_ops_test.sh
```

On Linux, additionally run `systemd-analyze verify` against both units after supplying their
referenced users/files in an isolated verifier. Installation, firewall changes, webhook
registration, credential rotation and process starts are explicit operator writes and are not
performed by repository validation.
