# Single-host deployment

This profile follows `ratatoskr-workspace/docs/DEPLOYMENT_TARGET.md` and the workspace port
contract: the webhook listens on loopback `8182` behind the
trusted cloudflared path, while operator listeners use `9467` (webhook) and `9468` (dispatcher).
Never publish either operator port through ingress. The units permit the outbound HTTPS required
by the configured Bot API and Platform endpoints: systemd's `IPAddressDeny`/`IPAddressAllow`
directives are bidirectional, so a private-only allowlist would also block those runtime
dependencies. Listener bind addresses, the trusted cloudflared route, and the target-host firewall
remain authoritative for ingress. A deployment that needs narrower directional egress must enforce
it in a host network policy without pinning provider-controlled DNS answers in these shared units.

`/etc/ratatoskr` is `0750 root:ratatoskr` and `/mnt/nvme/ratatoskr/logs` is `0770 root:ratatoskr`,
as ratatoskr-platform's `deploy/README.md` creates them; both units name `ratatoskr` in
`SupplementaryGroups=`. The webhook unit's `ReadWritePaths=` includes `/var/lib/ratatoskr-telegram`,
which systemd refuses to start without, so it is created below.

Install the binaries in `/usr/local/bin`, the two examples in `/etc/ratatoskr`, three root-owned
secret files under `/etc/ratatoskr/secrets`, the dispatcher's NATS seed at
`/etc/ratatoskr/telegram.nkey` (where Platform's NATS README and every other service of the fleet
install theirs, `/etc/ratatoskr/<identity>.nkey`), the logrotate policy in `/etc/logrotate.d`, and
the units in `/etc/systemd/system`. Environment files are `root:<role-group> 0640`. The shared bot
token and Platform signing key are `root:ratatoskr-telegram-secrets 0640`; both service users are
members of that narrow group. The webhook secret is `root:ratatoskr-telegram-webhook 0640`, and the
NATS seed is `root:ratatoskr-telegram-dispatcher 0640`. Directories remain root-owned and
not group-writable. Logs stay on NVMe under `/mnt/nvme/ratatoskr/logs`.

Both examples target the allocated Edge: `RATATOSKR__PLATFORM__AUDIENCE=edge` (the fixed audience of the Edge binary) and `RATATOSKR__PLATFORM__BASE_URL=http://127.0.0.1:8080` (Edge's public API listener in the workspace port allocation).

The binaries are never built on the host. Every commit on main publishes them, after the gate and
the arm64 smoke test (`deploy/tests/artifact_smoke.sh`), as
`ghcr.io/po4yka/ratatoskr-telegram:sha-<commit>` with `SHA256SUMS` and a provenance attestation, and
`ratatoskr-workspace`'s `deploy/ansible` playbook installs exactly that. By hand, verify it on a
machine with `oras` and `gh`, then copy `out/` to the host:

```console
$ oras pull "ghcr.io/po4yka/ratatoskr-telegram:sha-$COMMIT" -o out
$ gh attestation verify "oci://ghcr.io/po4yka/ratatoskr-telegram:sha-$COMMIT" \
    --repo po4yka/ratatoskr-telegram --source-digest "$COMMIT" \
    --signer-workflow po4yka/ratatoskr-workspace/.github/workflows/reusable-release-artifact.yml
$ (cd out && sha256sum -c SHA256SUMS)
```

After creating the named service users/groups and provisioning PostgreSQL/NATS under the workspace
profile, an authorized operator installs the non-secret artifacts explicitly:

```console
$ sudo getent group ratatoskr >/dev/null || sudo groupadd --system ratatoskr
$ sudo install -d -o root -g ratatoskr -m 0750 /etc/ratatoskr
$ sudo install -d -o root -g root -m 0755 /etc/ratatoskr/secrets
$ sudo install -d -o root -g ratatoskr -m 0770 /mnt/nvme/ratatoskr/logs
$ sudo install -d -o ratatoskr-telegram-webhook -g ratatoskr-telegram-webhook -m 0750 /var/lib/ratatoskr-telegram
$ sudo groupadd --system ratatoskr-telegram-blobs
$ sudo install -d -o ratatoskr-telegram-webhook -g ratatoskr-telegram-blobs -m 2750 /mnt/nvme/ratatoskr/blobs/ratatoskr-telegram
$ sudo install -o root -g root -m 0755 out/ratatoskr-telegram-webhook /usr/local/bin/
$ sudo install -o root -g root -m 0755 out/ratatoskr-telegram-dispatcher /usr/local/bin/
$ sudo install -o root -g root -m 0644 deploy/systemd/*.service /etc/systemd/system/
$ sudo install -o root -g root -m 0644 deploy/logrotate/ratatoskr-telegram /etc/logrotate.d/
$ sudo install -o root -g ratatoskr-telegram-webhook -m 0640 deploy/systemd/webhook.conf.example /etc/ratatoskr/telegram-webhook.conf
$ sudo install -o root -g ratatoskr-telegram-dispatcher -m 0640 deploy/systemd/dispatcher.conf.example /etc/ratatoskr/telegram-dispatcher.conf
```

Install the seed that Platform issued for the `ratatoskr-telegram` NATS identity (the source path is wherever the operator holds it, never in this repository):

```console
$ sudo install -m 0640 -o root -g ratatoskr-telegram-dispatcher <issued-telegram-seed-file> /etc/ratatoskr/telegram.nkey
```

Replace every `CHANGE-ME`, provision the three secret files under `/etc/ratatoskr/secrets` with the ownership described above,
and let Platform provision the fixed `ratatoskr_telegram_notifications` durable before enabling
the dispatcher. Validate both effective configurations and the fresh schema contract without
binding listeners:

```console
$ sudo -u ratatoskr-telegram-webhook /usr/local/bin/ratatoskr-telegram-webhook check-config
$ sudo -u ratatoskr-telegram-webhook /usr/local/bin/ratatoskr-telegram-webhook check-schema
$ sudo -u ratatoskr-telegram-dispatcher /usr/local/bin/ratatoskr-telegram-dispatcher check-config
$ sudo -u ratatoskr-telegram-dispatcher /usr/local/bin/ratatoskr-telegram-dispatcher check-schema
```

Both units are `WantedBy=` and `PartOf=ratatoskr.target`, which `ratatoskr-platform` installs and
enables (its `deploy/README.md`). Without that target they start now but not at boot, and
`sudo systemctl stop ratatoskr.target` stops them with the rest of the deployment.

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

### Attachment blob root

The webhook stages PDF attachments in `/mnt/nvme/ratatoskr/blobs/ratatoskr-telegram` (`RATATOSKR__INGESTION__BLOB_ROOT` in the webhook example), on the durable device rather than the boot device. The unit lists the path in `ReadWritePaths`, runs with `UMask=0027`, and carries `ratatoskr-telegram-blobs` in `SupplementaryGroups`. The directory is created with `install -d -o ratatoskr-telegram-webhook -g ratatoskr-telegram-blobs -m 2750` as shown above: the setgid bit makes every new blob file inherit the group, and the mode gives that group read and traverse access only.

The extractor runs with `SupplementaryGroups=ratatoskr-telegram-blobs` and `ReadOnlyPaths=/mnt/nvme/ratatoskr/blobs/ratatoskr-telegram`, and reads Telegram-owned bytes by `BlobRef` (owner, sha256 digest, length). It never writes, renames, or deletes there, and no filesystem path crosses a service boundary. If the group does not exist or the extractor is not a member, its runs fail with `blob_unreadable`; that is host state no repository can apply.

### PostgreSQL role

Both services connect to Ratatoskr's one database, `ratatoskr` on port 5432, as the role `ratatoskr_telegram`; their `telegram` schema is created by the webhook's `check-schema` step. Provision in this order: Edge first, then `deploy/postgres/role.sql`, then the Telegram services. Platform's `01-database-and-roles.sql` creates the database, Edge is started once, and Platform's `02-grants.sql` follows; `role.sql` is then run once by a PostgreSQL administrator against that database, and the password is set separately so that no credential enters the repository:

```console
$ sudo -u postgres psql --file deploy/postgres/role.sql
$ sudo -u postgres psql --command '\password ratatoskr_telegram'
```

Put the same password in `RATATOSKR__DATABASE__URL` of both configuration files in place of `CHANGE-ME`. The role cannot create databases or roles, has no rights on schema `public`, and may create only its own schema.

### Platform assertion key

The webhook and the dispatcher sign short-lived identity assertions with one Ed25519 seed, and Edge verifies them with the matching public key. The seed is a file of 64 lowercase hexadecimal characters and one newline (`RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY_FILE`); the public key is the base64 (standard alphabet, padded) form that Edge reads as `RATATOSKR__IDENTITY__ASSERTION_KEY`. Without that variable Edge leaves `telegram.mini_app` unavailable and `/v1/sessions/telegram` refuses every exchange with 401. The key is generated once, with the webhook binary; the dispatcher binary has no such command and reads the same file through the shared secrets group.

```console
$ sudo /usr/local/bin/ratatoskr-telegram-webhook assertion-key generate /etc/ratatoskr/secrets/telegram-platform-signing-key
$ sudo chown root:ratatoskr-telegram-secrets /etc/ratatoskr/secrets/telegram-platform-signing-key
$ sudo chmod 0640 /etc/ratatoskr/secrets/telegram-platform-signing-key
```

`generate` creates the file exclusively with mode `0600`, refuses to replace an existing file, and prints only the public key; the final two commands give the seed its deployed ownership, `root:ratatoskr-telegram-secrets 0640`. To print the public key of an installed seed again, or to derive it on another host:

```console
$ sudo /usr/local/bin/ratatoskr-telegram-webhook assertion-key public /etc/ratatoskr/secrets/telegram-platform-signing-key
```

Set the printed line as `RATATOSKR__IDENTITY__ASSERTION_KEY=<public key>` in the Edge configuration file, `/etc/ratatoskr/edge.conf`, and restart Edge. Neither command ever prints the seed. Rotating the key means removing the old seed file deliberately, running `generate` again and updating `/etc/ratatoskr/edge.conf` in the same step, because Edge refuses assertions signed by a seed it no longer has the public half of.

### Notification ack wait

Edge provisions the `ratatoskr_telegram_notifications` durable consumer with an ack wait of 30 seconds, and the dispatcher verifies that durable at startup without ever repairing it. `RATATOSKR__NOTIFICATION_BUS__ACK_WAIT_SECONDS` therefore exists only to state that value: it must be 30, any other value stops the dispatcher at configuration validation, and the example sets it to 30 for that reason.
