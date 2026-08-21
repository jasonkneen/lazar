#!/usr/bin/env bash
# Offline discrimination and timeout tests. No provider or network is used.
set -uo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
SKILL="${HARNESS_EVALS_SKILL_DIR:-$LAZAR_HOME/user/skills/harness-evals}"
PYTHON="${HARNESS_EVALS_PYTHON:-python3}"
SCORER="$SKILL/lib/score_stream.py"
TIMEOUT_RUNNER="$SKILL/lib/run_with_timeout.py"
FIXTURES="$SKILL/fixtures"
TEST_PARENT="${TMPDIR:-/tmp}"
TEST_PARENT="${TEST_PARENT%/}"
TEST_ROOT=$(mktemp -d "$TEST_PARENT/lazar-harness-evals-selftest.XXXXXX")
PASS=0
FAIL=0

cleanup() {
    case "$TEST_ROOT" in
        "$TEST_PARENT"/lazar-harness-evals-selftest.*|/tmp/lazar-harness-evals-selftest.*|/private/tmp/lazar-harness-evals-selftest.*)
            rm -rf -- "$TEST_ROOT"
            ;;
        *) echo "refusing unsafe cleanup target: $TEST_ROOT" >&2 ;;
    esac
}
trap cleanup EXIT

ok() { echo "  [PASS] $1"; PASS=$((PASS + 1)); }
no() { echo "  [FAIL] $1"; FAIL=$((FAIL + 1)); }

status_of() {
    "$PYTHON" "$SCORER" "$1" --task "$2" --ctx "$3" 2>/dev/null |
        "$PYTHON" -c 'import json,sys; print(json.load(sys.stdin)["status"])'
}

CONTEXT='{"token":"ABC","workdir":"'"$TEST_ROOT"'","lazar_home":"'"$LAZAR_HOME"'"}'

echo "== harness-evals selftest (offline) =="

[[ "$(status_of "$FIXTURES/genuine_tool.jsonl" "$FIXTURES/task_tool.json" "$CONTEXT")" == "pass" ]] \
    && ok "real tool event and result pass" || no "real tool transcript should pass"

[[ "$(status_of "$FIXTURES/narrated_only.jsonl" "$FIXTURES/task_tool.json" "$CONTEXT")" == "fail" ]] \
    && ok "narrated tool use fails" || no "assistant prose must not substitute for a tool event"

[[ "$(status_of "$FIXTURES/clean_chat.jsonl" "$FIXTURES/task_chat.json" "$CONTEXT")" == "pass" ]] \
    && ok "clean terminal chat passes" || no "clean chat should pass"

[[ "$(status_of "$FIXTURES/truncated.jsonl" "$FIXTURES/task_chat.json" "$CONTEXT")" == "error" ]] \
    && ok "truncated stream is infrastructure error" || no "truncated stream should be error"

[[ "$(status_of "$FIXTURES/invalid_stream.jsonl" "$FIXTURES/task_chat.json" "$CONTEXT")" == "error" ]] \
    && ok "invalid JSONL is infrastructure error" || no "invalid stream must not partially pass"

cat >"$TEST_ROOT/max-tokens.jsonl" <<'EOF'
{"type":"text_delta","text":"present"}
{"type":"invoke_end","stop_reason":"max_tokens"}
EOF
cat >"$TEST_ROOT/no-terminal-check.json" <<'EOF'
{"id":"terminal-global","suite":"selftest","title":"terminal gate","checks":[{"type":"text_contains","needle":"present"}]}
EOF
[[ "$(status_of "$TEST_ROOT/max-tokens.jsonl" "$TEST_ROOT/no-terminal-check.json" "$CONTEXT")" == "fail" ]] \
    && ok "non-success terminal reason fails globally" || no "terminal success must not depend on a task-local check"

[[ "$(status_of "$FIXTURES/recovered.jsonl" "$FIXTURES/task_recovery.json" "$CONTEXT")" == "pass" ]] \
    && ok "failed tool followed by successful recovery passes" || no "recovery transcript should pass"

printf '%s\n' 'ARTIFACT_ABC' >"$TEST_ROOT/artifact.txt"
[[ "$(status_of "$FIXTURES/artifact_claim.jsonl" "$FIXTURES/task_artifact.json" "$CONTEXT")" == "pass" ]] \
    && ok "matching filesystem artifact passes" || no "matching artifact should pass"
rm "$TEST_ROOT/artifact.txt"
[[ "$(status_of "$FIXTURES/artifact_claim.jsonl" "$FIXTURES/task_artifact.json" "$CONTEXT")" == "fail" ]] \
    && ok "missing artifact fails despite success prose" || no "prose must not override missing filesystem state"

if "$PYTHON" "$TIMEOUT_RUNNER" --timeout 0.05 \
    --stdout "$TEST_ROOT/timeout.out" --stderr "$TEST_ROOT/timeout.err" \
    -- "$PYTHON" -c 'import time; time.sleep(5)' \
    >/dev/null 2>&1; then
    timeout_rc=0
else
    timeout_rc=$?
fi
[[ "$timeout_rc" == "124" ]] \
    && ok "portable timeout terminates a stuck adapter" || no "timeout returned $timeout_rc, expected 124"

FAKE_AGENT="$TEST_ROOT/fake-agent"
cat >"$FAKE_AGENT" <<'EOF'
#!/usr/bin/env bash
set -u
[[ $# -eq 1 ]] || exit 9
token=$(printf '%s\n' "$1" | sed -nE 's/.*(CHAT_OK_[A-F0-9]+).*/\1/p')
[[ -n "$token" ]] || exit 8
printf '%s\n' \
    '{"type":"invoke_start","model":"fake"}' \
    "{\"type\":\"text_delta\",\"text\":\"$token\"}" \
    '{"type":"invoke_end","stop_reason":"end_turn"}'
EOF
chmod +x "$FAKE_AGENT"
if HARNESS_EVALS_RESULTS="$TEST_ROOT/runner-results" \
    bash "$SKILL/bin/harness-evals" run --suite core --task core.chat \
    --adapter command --cmd "$FAKE_AGENT {{prompt}}" \
    >"$TEST_ROOT/runner.out" 2>"$TEST_ROOT/runner.err"; then
    summary=$(find "$TEST_ROOT/runner-results" -name summary.json -type f | head -n 1)
    if [[ -n "$summary" ]] && [[ "$(jq -r '.pass == 1 and .fail == 0 and .error == 0' "$summary")" == "true" ]]; then
        ok "command adapter preserves one quoted prompt argument"
    else
        no "command adapter run produced no passing summary"
    fi
else
    no "command adapter runner failed"
fi

if HARNESS_EVALS_RESULTS="$TEST_ROOT/unsafe-results" \
    bash "$SKILL/bin/harness-evals" run --suite core --task core.chat \
    --adapter command --cmd "$FAKE_AGENT {{prompt}}" --label ../escape \
    >"$TEST_ROOT/unsafe.out" 2>"$TEST_ROOT/unsafe.err"; then
    no "unsafe run label unexpectedly passed"
else
    grep -q -- '--label must be a safe slug' "$TEST_ROOT/unsafe.err" \
        && ok "unsafe result label is refused" || no "unsafe label lacked a clear refusal"
fi

TRUNCATED_AGENT="$TEST_ROOT/truncated-agent"
cat >"$TRUNCATED_AGENT" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' \
    '{"type":"invoke_start","model":"fake"}' \
    '{"type":"text_delta","text":"CHAT_OK_BADC0DE"}'
EOF
chmod +x "$TRUNCATED_AGENT"
if HARNESS_EVALS_RESULTS="$TEST_ROOT/truncated-results" \
    bash "$SKILL/bin/harness-evals" run --suite core --task core.chat \
    --adapter command --cmd "$TRUNCATED_AGENT {{prompt}}" \
    >"$TEST_ROOT/truncated-runner.out" 2>"$TEST_ROOT/truncated-runner.err"; then
    no "required incomplete stream unexpectedly passed the runner gate"
else
    summary=$(find "$TEST_ROOT/truncated-results" -name summary.json -type f | head -n 1)
    if [[ -n "$summary" ]] && [[ "$(jq -r '.error == 1' "$summary")" == "true" ]]; then
        ok "required incomplete stream fails the runner gate"
    else
        no "incomplete runner failure lacked an error summary"
    fi
fi

for task_file in "$SKILL"/tasks/*/tasks.jsonl; do
    count=0
    bad=0
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        count=$((count + 1))
        printf '%s\n' "$line" |
            "$PYTHON" -c 'import json,sys; d=json.load(sys.stdin); assert d["id"] and d["checks"]' \
            2>/dev/null || bad=$((bad + 1))
    done <"$task_file"
    [[ "$bad" == "0" ]] \
        && ok "task pack valid: $(basename "$(dirname "$task_file")") ($count tasks)" \
        || no "task pack has $bad invalid rows: $task_file"
done

echo
echo "selftest: PASS=$PASS FAIL=$FAIL"
[[ "$FAIL" == "0" ]]
