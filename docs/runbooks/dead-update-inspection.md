# Dead update and notification inspection

Preview the bounded, content-free projection:

```console
$ deploy/bin/telegram-ops inspect-dead --kind all --limit 25 --dry-run
```

Read the live Telegram-owned rows only after supplying the database URL out of shell history:

```console
$ sudo --preserve-env=RATATOSKR_TELEGRAM_DATABASE_URL deploy/bin/telegram-ops inspect-dead --kind all --limit 25
```

The projection contains identifiers, timestamps, attempts, safe failure classes and opaque
correlation references only. It excludes update payloads, message text, titles, usernames, chat
ids, credentials and provider diagnostics. Inspection never retries or deletes a row; use the
state-guarded recovery workflow only for a confirmed stuck update.
