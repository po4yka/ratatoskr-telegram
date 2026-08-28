# Session and stuck-operation recovery

Platform owns Ratatoskr sessions and operation status. Telegram may identify the affected user but
must inspect or mutate sessions only through Platform's authenticated operator surface:

```console
$ deploy/bin/telegram-ops inspect-session --user-ref user:018f65d8-25a1-7f59-aaf8-72941f37c031 --dry-run
```

For an update left `processing` after a crash, inspect the safe update identifier and timestamp,
confirm the row still has its minimized processable payload, then preview the exact conditional
recovery:

```console
$ deploy/bin/telegram-ops recover-stuck-update --bot-id 700100200 --update-id 42 --expected-state processing --dry-run
```

Execute only after approval and with the Telegram database URL supplied out of shell history:

```console
$ sudo --preserve-env=RATATOSKR_TELEGRAM_DATABASE_URL deploy/bin/telegram-ops recover-stuck-update --bot-id 700100200 --update-id 42 --expected-state processing --execute --ack 'recover 700100200/42 from processing'
```

The update is conditional and affects at most one row. `refused_state_mismatch` means stop: another
worker or operator changed the state. Domain-operation retries remain Platform actions and are not
implemented by this tool.
