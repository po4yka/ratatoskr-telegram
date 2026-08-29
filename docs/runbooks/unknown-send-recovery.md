# Unknown Telegram send recovery

An `outcome_unknown` job may already have been applied by Telegram. Ordinary dispatcher claims do
not replay it. Inspect only safe identifiers and closed error classes first:

```bash
$ deploy/bin/telegram-ops inspect-dead --kind outbound --limit 25 --dry-run
$ sudo --preserve-env=RATATOSKR_TELEGRAM_DATABASE_URL deploy/bin/telegram-ops inspect-dead --kind outbound --limit 25
```

If the user-visible cost of a missing message outweighs the duplicate risk, preview an explicit
new attempt. The original job remains quarantined as audit evidence:

```bash
$ deploy/bin/telegram-ops recover-unknown-send --job-id 018f65d8-25a1-7f59-aaf8-72941f37c031 --expected-state outcome_unknown --dry-run
```

Execution transactionally rechecks the exact state and kind. The acknowledgement is deliberately
specific because Telegram has no idempotency key or reconciliation lookup for `sendMessage`:

```bash
$ sudo --preserve-env=RATATOSKR_TELEGRAM_DATABASE_URL deploy/bin/telegram-ops recover-unknown-send --job-id 018f65d8-25a1-7f59-aaf8-72941f37c031 --expected-state outcome_unknown --execute --ack 'resend unknown 018f65d8-25a1-7f59-aaf8-72941f37c031 accepting duplicate risk'
```
