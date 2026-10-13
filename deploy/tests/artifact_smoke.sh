#!/usr/bin/env bash
# The published binaries, started once each on the architecture they ship to.
#
# Usage: deploy/tests/artifact_smoke.sh <runtime-image>
#
# For each role: `check-config`, start, `/health/ready` answers 200, SIGTERM, exit 0. The webhook
# first runs `check-schema`, which creates the `telegram` schema as the deploy guide says, and the
# dispatcher then only verifies it.
#
# The dependencies are real: PostgreSQL 17 with the ICU collation, and a NATS broker that enforces
# the Telegram identity exactly as `deploy/nats/identity.conf` ships it. An administrator identity
# stands in for Edge and declares `ratatoskr_events` and the `ratatoskr_telegram_notifications`
# durable, as Edge does on the host. The Bot API is a loopback stand-in that answers `getMe` with
# the same fixture the boot tests use, because the webhook calls it at startup and a smoke test
# must not reach `api.telegram.org`.
#
# Every container joins the network namespace of the database container, so every address is
# 127.0.0.1 as in a unit file, on a CI runner and on a developer's Docker Desktop alike. The keys are
# generated for this run and deleted with it; they never protect anything.
set -euo pipefail

image="${1:?usage: $0 <runtime-image>}"
repo="$(cd "$(dirname "$0")/../.." && pwd)"

POSTGRES="postgres:17@sha256:e38411452a464af89e5adadb8d223bf53b898d47d6ef918b2d58c08707350449"
NATS="nats:2.12-alpine@sha256:b270f5e2428354c0335612694d7dd2fb588148e567a5757fdff325ef9c9332e6"
NATS_BOX="natsio/nats-box:0.19.7@sha256:ffce8bd103383f179f8c7f11cf645726acf5d17280706c530c3b342dbe16334c"
CURL="curlimages/curl:8.11.1@sha256:c1fe1679c34d9784c1b0d1e5f62ac0a79fca01fb6377cdd33e90473c6f9f9a69"
PYTHON="python:3.13-slim-bookworm@sha256:a1165e272e578941b84abc79e4ab38a0305cd12803a5c4247979ac7655f4d641"

PLACEHOLDER="UREPLACE_ME_WITH_THE_PUBLIC_NKEY_OF_RATATOSKR_TELEGRAM_XXXX"
CONTAINERS=(smoke-webhook smoke-dispatcher smoke-botapi smoke-bus smoke-db)

# A previous run that was killed leaves its containers behind; remove them first.
docker rm -f "${CONTAINERS[@]}" >/dev/null 2>&1 || true
work="$(mktemp -d)"
cleanup() {
  docker rm -f "${CONTAINERS[@]}" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

in_ns() { docker run --rm --network container:smoke-db "$@"; }
status_of() { in_ns "$CURL" -s -o /dev/null -w '%{http_code}' "$1" || true; }

wait_for() { # wait_for <description> <url>
  for _ in $(seq 1 60); do
    if [ "$(status_of "$2")" = 200 ]; then return 0; fi
    sleep 1
  done
  echo "::error::$1 did not answer 200 at $2 within 60 s" >&2
  return 1
}

stop_cleanly() { # stop_cleanly <container>
  docker stop -t 40 "$1" >/dev/null
  local code
  code="$(docker inspect -f '{{.State.ExitCode}}' "$1")"
  if [ "$code" != 0 ]; then
    docker logs "$1" >&2
    echo "::error::$1 exited $code after SIGTERM" >&2
    return 1
  fi
  echo "$1: ready, then exit 0 after SIGTERM"
}

# --- PostgreSQL --------------------------------------------------------------------------------
docker run -d --name smoke-db \
  -e POSTGRES_USER=telegram -e POSTGRES_PASSWORD=telegram -e POSTGRES_DB=telegram \
  -e POSTGRES_INITDB_ARGS="--locale-provider=icu --icu-locale=und-x-icu --encoding=UTF8" \
  "$POSTGRES" >/dev/null

# --- NATS, with the shipped Telegram stanza ----------------------------------------------------
docker run --rm "$NATS_BOX" nk -gen user -pubout >"$work/admin.pair"
docker run --rm "$NATS_BOX" nk -gen user -pubout >"$work/telegram.pair"
sed -n 1p "$work/admin.pair" >"$work/admin.nkey"
sed -n 1p "$work/telegram.pair" >"$work/telegram.nkey"
# Read by the dispatcher, which runs as an unprivileged user in its image.
chmod 0644 "$work/telegram.nkey" "$work/admin.nkey"
stanza="$(sed "s/$PLACEHOLDER/$(sed -n 2p "$work/telegram.pair")/" "$repo/deploy/nats/identity.conf")"
cat >"$work/nats.conf" <<CONF
port: 4222
host: 0.0.0.0
http_port: 8222
jetstream { store_dir: /data }
authorization {
  users: [
    { nkey: $(sed -n 2p "$work/admin.pair") },
    $stanza
  ]
}
CONF
docker run -d --name smoke-bus --network container:smoke-db \
  -v "$work/nats.conf:/etc/nats/nats.conf:ro" "$NATS" -c /etc/nats/nats.conf >/dev/null

for _ in $(seq 1 60); do
  if docker exec smoke-db pg_isready -h 127.0.0.1 -U telegram -d telegram >/dev/null 2>&1; then break; fi
  sleep 1
done
wait_for "NATS" http://127.0.0.1:8222/healthz

nats_admin() { in_ns -v "$work:/w:ro" "$NATS_BOX" nats --server nats://127.0.0.1:4222 --nkey /w/admin.nkey "$@"; }
nats_admin stream add ratatoskr_events --subjects 'evt.>' --defaults >/dev/null
nats_admin consumer add ratatoskr_events ratatoskr_telegram_notifications --pull \
  --filter evt.platform.notification.raised.v1 --ack explicit --wait 30s --defaults >/dev/null

# --- The Bot API stand-in ----------------------------------------------------------------------
cp "$repo/crates/bot-api/tests/fixtures/get_me.json" "$work/get_me.json"
cat >"$work/botapi.py" <<'PY'
import http.server, pathlib

GET_ME = pathlib.Path("/w/get_me.json").read_bytes()

class Handler(http.server.BaseHTTPRequestHandler):
    def answer(self):
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        if self.path.lower().endswith("/getme"):
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(GET_ME)))
            self.end_headers()
            self.wfile.write(GET_ME)
        else:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()

    do_GET = do_POST = answer

http.server.HTTPServer(("127.0.0.1", 18081), Handler).serve_forever()
PY
docker run -d --name smoke-botapi --network container:smoke-db -v "$work:/w:ro" \
  "$PYTHON" python3 /w/botapi.py >/dev/null
wait_for "the Bot API stand-in" http://127.0.0.1:18081/bot0/getMe

# --- The two roles -----------------------------------------------------------------------------
common=(
  -e RATATOSKR__DATABASE__URL=postgres://telegram:telegram@127.0.0.1:5432/telegram
  -e RATATOSKR__BOT_API__BASE_URL=http://127.0.0.1:18081
  -e RATATOSKR__BOT_API__TOKEN=123456:SMOKE-artifact-token
  -e RATATOSKR__PLATFORM__BASE_URL=http://127.0.0.1:8080
  -e RATATOSKR__PLATFORM__AUDIENCE=edge
  -e RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY=0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20
  -e RATATOSKR__TELEMETRY__LOG_FORMAT=json
)
webhook=(
  "${common[@]}"
  -e RATATOSKR__ADMIN__BIND=127.0.0.1:9467
  -e RATATOSKR__WEBHOOK__BIND=127.0.0.1:8182
  -e RATATOSKR__WEBHOOK__SECRET_TOKEN=smoke-artifact-secret-token
  -e RATATOSKR__ACCESS__OWNER_TELEGRAM_USER_ID=700100200
  -e RATATOSKR__INGESTION__BLOB_ROOT=/blobs
)
dispatcher=(
  "${common[@]}"
  -v "$work/telegram.nkey:/etc/ratatoskr/telegram.nkey:ro"
  -e RATATOSKR__ADMIN__BIND=127.0.0.1:9468
  -e RATATOSKR__NOTIFICATION_BUS__ENDPOINT=nats://127.0.0.1:4222
  -e RATATOSKR__NOTIFICATION_BUS__CREDENTIALS_FILE=/etc/ratatoskr/telegram.nkey
)

in_ns --tmpfs /blobs:mode=1777 "${webhook[@]}" "$image" ratatoskr-telegram-webhook check-config >/dev/null
in_ns --tmpfs /blobs:mode=1777 "${webhook[@]}" "$image" ratatoskr-telegram-webhook check-schema >/dev/null
docker run -d --name smoke-webhook --network container:smoke-db --tmpfs /blobs:mode=1777 \
  "${webhook[@]}" "$image" ratatoskr-telegram-webhook >/dev/null
wait_for "the webhook" http://127.0.0.1:9467/health/ready || { docker logs smoke-webhook >&2; exit 1; }
stop_cleanly smoke-webhook

in_ns "${dispatcher[@]}" "$image" ratatoskr-telegram-dispatcher check-config >/dev/null
docker run -d --name smoke-dispatcher --network container:smoke-db \
  "${dispatcher[@]}" "$image" ratatoskr-telegram-dispatcher >/dev/null
wait_for "the dispatcher" http://127.0.0.1:9468/health/ready || { docker logs smoke-dispatcher >&2; exit 1; }
stop_cleanly smoke-dispatcher
