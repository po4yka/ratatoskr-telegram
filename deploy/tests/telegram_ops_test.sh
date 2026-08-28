#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OPS="$ROOT/deploy/bin/telegram-ops"

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

rotation_dry_runs_are_redacted_and_non_mutating() {
    local work candidate before after output
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' RETURN
    candidate="$work/candidate"
    printf '%s\n' 'synthetic_secret_value_never_print' > "$candidate"
    before="$(cksum "$candidate")"
    output="$("$OPS" rotate-webhook-secret --candidate "$candidate" --dry-run)"
    after="$(cksum "$candidate")"
    [[ "$before" == "$after" ]] || fail 'dry-run changed the candidate'
    [[ "$output" == *'DRY-RUN'* ]] || fail 'dry-run marker missing'
    [[ "$output" != *'synthetic_secret_value_never_print'* ]] || fail 'secret leaked'
    printf '%s\n' '123456:synthetic_bot_token_value' > "$candidate"
    output="$("$OPS" rotate-bot-token --candidate "$candidate" --dry-run)"
    [[ "$output" == *'DRY-RUN'* ]] || fail 'bot-token dry-run marker missing'
    [[ "$output" != *'synthetic_bot_token_value'* ]] || fail 'bot token leaked'
}

rotation_execute_and_readiness_rollback_use_only_local_fixture_tools() {
    local work fake candidate destination output restored
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' RETURN
    fake="$work/bin"
    mkdir "$fake"
    cat > "$fake/systemctl" <<'EOF'
#!/usr/bin/env bash
[[ "$1" == restart ]]
EOF
    cat > "$fake/curl" <<'EOF'
#!/usr/bin/env bash
[[ "${FAKE_CURL_FAIL:-false}" != true ]]
EOF
    chmod 0755 "$fake/systemctl" "$fake/curl"
    candidate="$work/candidate"
    destination="$work/destination"
    printf '%s\n' 'old_synthetic_secret_value' > "$destination"
    printf '%s\n' 'new_synthetic_secret_value' > "$candidate"
    output="$(PATH="$fake:$PATH" "$OPS" rotate-webhook-secret --candidate "$candidate" \
        --destination "$destination" --execute --ack 'rotate webhook-secret credential')"
    [[ "$(<"$destination")" == new_synthetic_secret_value ]] || fail 'execute did not install candidate'
    [[ "$(<"$destination.previous")" == old_synthetic_secret_value ]] || fail 'previous value not retained'
    [[ "$output" != *'new_synthetic_secret_value'* ]] || fail 'execute leaked secret'

    printf '%s\n' 'rollback_candidate_secret_value' > "$candidate"
    if PATH="$fake:$PATH" FAKE_CURL_FAIL=true "$OPS" rotate-webhook-secret \
        --candidate "$candidate" --destination "$destination" --execute \
        --ack 'rotate webhook-secret credential' >/dev/null 2>&1; then
        fail 'readiness failure did not fail rotation'
    fi
    restored="$(<"$destination")"
    [[ "$restored" == new_synthetic_secret_value ]] || fail 'readiness failure did not roll back'

    printf '%s\n' '123456:another_synthetic_bot_token' > "$candidate"
    PATH="$fake:$PATH" "$OPS" rotate-bot-token --candidate "$candidate" \
        --destination "$destination" --execute --ack 'rotate bot-token credential' >/dev/null
    [[ "$(<"$destination")" == '123456:another_synthetic_bot_token' ]] || fail 'bot token execute failed'
}

session_inspection_uses_platform_authority() {
    local output
    output="$("$OPS" inspect-session --user-ref user:018f65d8-25a1-7f59-aaf8-72941f37c031 --dry-run)"
    [[ "$output" == *'Platform operator surface'* ]] || fail 'session authority is not Platform'
    [[ "$output" != *'update platform.'* ]] || fail 'cross-schema mutation emitted'
}

stuck_recovery_requires_expected_state_and_execute() {
    if "$OPS" recover-stuck-update --bot-id 700100200 --update-id 42 --expected-state processing >/dev/null 2>&1; then
        fail 'recovery mutated without --execute'
    fi
    local output
    output="$("$OPS" recover-stuck-update --bot-id 700100200 --update-id 42 --expected-state processing --dry-run)"
    [[ "$output" == *'expected state: processing'* ]] || fail 'expected-state guard missing'
    [[ "$output" == *'at most one row'* ]] || fail 'one-row bound missing'
}

dead_inspection_is_bounded_and_read_only() {
    local output
    output="$("$OPS" inspect-dead --kind all --limit 25 --dry-run)"
    [[ "$output" == *'identifiers,timestamps,attempts,safe_class,correlation_ref'* ]] || fail 'safe projection absent'
    [[ "$output" != *'payload'* && "$output" != *'chat_id'* && "$output" != *'title'* ]] || fail 'private column exposed'
}

runbook_commands_execute_as_written() {
    for runbook in "$ROOT"/docs/runbooks/*.md; do
        [[ -f "$runbook" ]] || fail 'runbooks absent'
        while IFS= read -r command; do
            [[ -n "$command" ]] || continue
            bash -n <<<"$command"
        done < <(sed -n 's/^\$ //p' "$runbook")
    done
}

rotation_dry_runs_are_redacted_and_non_mutating
rotation_execute_and_readiness_rollback_use_only_local_fixture_tools
session_inspection_uses_platform_authority
stuck_recovery_requires_expected_state_and_execute
dead_inspection_is_bounded_and_read_only
runbook_commands_execute_as_written
printf 'PASS: telegram operator dry-run and runbook contract\n'
