# Webhook secret rotation

Validate the candidate and local replacement plan without reading or printing its value:

```console
$ deploy/bin/telegram-ops rotate-webhook-secret --candidate /run/credentials/webhook.next --dry-run
```

The provider write is separate authority: register the candidate secret with Telegram only after
the dry run and an explicit change approval. Then perform the atomic local replacement:

```console
$ sudo deploy/bin/telegram-ops rotate-webhook-secret --candidate /run/credentials/webhook.next --destination /etc/ratatoskr/secrets/telegram-webhook --execute --ack 'rotate webhook-secret credential'
```

The tool restarts only the webhook role and checks `127.0.0.1:9467/health/ready`. If readiness
fails, restore the printed `.previous` path atomically, restart the webhook, verify readiness, and
re-register the previous provider secret under separate approval. Never revoke the previous secret
before both sides are verified.
