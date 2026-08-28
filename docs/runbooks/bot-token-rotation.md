# Bot token rotation

BotFather token revocation/creation is a credential action outside this repository. After an
authorized operator places the candidate in a root-only temporary file, validate the local plan:

```console
$ deploy/bin/telegram-ops rotate-bot-token --candidate /run/credentials/bot-token.next --dry-run
```

Then replace locally with an explicit acknowledgement:

```console
$ sudo deploy/bin/telegram-ops rotate-bot-token --candidate /run/credentials/bot-token.next --destination /etc/ratatoskr/secrets/telegram-bot-token --execute --ack 'rotate bot-token credential'
```

Both roles restart because webhook intake and dispatcher delivery share this Telegram credential;
the tool checks ports 9467 and 9468. Re-register the webhook under separate provider authority if
the token rotation invalidated it. On failure, restore `.previous`, restart both roles, and verify
both readiness endpoints before investigating provider state.
