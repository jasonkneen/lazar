#!/usr/bin/env bash
# Offline behavior tests for bin/lazar-failover.
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
FAILOVER_BIN="${LAZAR_FAILOVER_BIN:-$LAZAR_HOME/bin/lazar-failover}"
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/lazar-failover-selftest.XXXXXX")

cleanup() {
    case "$TEST_ROOT" in
        "${TMPDIR:-/tmp}"/lazar-failover-selftest.*|/tmp/lazar-failover-selftest.*|/private/tmp/lazar-failover-selftest.*)
            rm -rf -- "$TEST_ROOT"
            ;;
        *) echo "refusing unsafe cleanup target: $TEST_ROOT" >&2 ;;
    esac
}
trap cleanup EXIT

FAKE_LAZAR="$TEST_ROOT/fake-lazar"
PROMPT_FILE="$TEST_ROOT/prompt.txt"
ATTEMPTS="$TEST_ROOT/attempts"

cat >"$FAKE_LAZAR" <<'EOF'
#!/usr/bin/env bash
set -u
if [[ "${1:-}" == "providers" && "${2:-}" == "--json" ]]; then
    printf '%s\n' '[{"id":"unkeyed","key_available":false},{"id":"first","key_available":true},{"id":"second","key_available":true}]'
    exit 0
fi
provider=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --provider) provider="$2"; shift 2 ;;
        *) shift ;;
    esac
done
printf '%s\n' "$provider" >>"$FAKE_ATTEMPTS"
if [[ "${FAKE_MODE:-safe}" == "empty" ]]; then
    exit 10
fi
printf '{"type":"invoke_start","provider":"%s"}\n' "$provider"
case "${FAKE_MODE:-safe}" in
    safe)
        if [[ "$provider" == "first" ]]; then
            echo "first unavailable" >&2
            exit 7
        fi
        printf '%s\n' '{"type":"text_delta","text":"ok"}' '{"type":"invoke_end","stop_reason":"end_turn"}'
        ;;
    side-effect)
        printf '%s\n' '{"type":"tool_use","id":"t1","name":"bash","input":{"command":"touch changed"}}'
        echo "connection dropped" >&2
        exit 8
        ;;
    invalid)
        printf '%s\n' 'not-json'
        exit 9
        ;;
    non-object)
        printf '%s\n' '[]'
        exit 11
        ;;
    success-invalid)
        printf '%s\n' 'not-json'
        exit 0
        ;;
esac
EOF
chmod +x "$FAKE_LAZAR"
printf '%s\n' 'Report status.' >"$PROMPT_FILE"

run_failover() {
    FAKE_ATTEMPTS="$ATTEMPTS" LAZAR_BIN="$FAKE_LAZAR" \
        "$FAILOVER_BIN" "$@"
}

: >"$ATTEMPTS"
run_failover "$PROMPT_FILE" first second >"$TEST_ROOT/safe.out" 2>"$TEST_ROOT/safe.err"
[[ "$(wc -l <"$ATTEMPTS" | tr -d '[:space:]')" == "2" ]]
grep -q '"provider":"second"' "$TEST_ROOT/safe.out"
! grep -q '"provider":"first"' "$TEST_ROOT/safe.out"

: >"$ATTEMPTS"
if FAKE_MODE=side-effect run_failover "$PROMPT_FILE" first second >"$TEST_ROOT/tool.out" 2>"$TEST_ROOT/tool.err"; then
    echo "side-effecting failure unexpectedly succeeded" >&2
    exit 1
fi
[[ "$(wc -l <"$ATTEMPTS" | tr -d '[:space:]')" == "1" ]]
grep -q '"type":"tool_use"' "$TEST_ROOT/tool.out"
grep -q 'could duplicate side effects' "$TEST_ROOT/tool.err"

: >"$ATTEMPTS"
if FAKE_MODE=invalid run_failover "$PROMPT_FILE" first second >"$TEST_ROOT/invalid.out" 2>"$TEST_ROOT/invalid.err"; then
    echo "invalid stream unexpectedly succeeded" >&2
    exit 1
fi
[[ "$(wc -l <"$ATTEMPTS" | tr -d '[:space:]')" == "1" ]]
grep -q 'invalid stream-json' "$TEST_ROOT/invalid.err"

: >"$ATTEMPTS"
if FAKE_MODE=empty run_failover "$PROMPT_FILE" first second >"$TEST_ROOT/empty.out" 2>"$TEST_ROOT/empty.err"; then
    echo "empty stream unexpectedly succeeded" >&2
    exit 1
fi
[[ "$(wc -l <"$ATTEMPTS" | tr -d '[:space:]')" == "1" ]]
grep -q 'invalid stream-json' "$TEST_ROOT/empty.err"

: >"$ATTEMPTS"
if FAKE_MODE=non-object run_failover "$PROMPT_FILE" first second >"$TEST_ROOT/non-object.out" 2>"$TEST_ROOT/non-object.err"; then
    echo "non-object stream unexpectedly succeeded" >&2
    exit 1
fi
[[ "$(wc -l <"$ATTEMPTS" | tr -d '[:space:]')" == "1" ]]
grep -q 'invalid stream-json' "$TEST_ROOT/non-object.err"

: >"$ATTEMPTS"
if FAKE_MODE=success-invalid run_failover "$PROMPT_FILE" first second >"$TEST_ROOT/success-invalid.out" 2>"$TEST_ROOT/success-invalid.err"; then
    echo "successful process with invalid stream unexpectedly succeeded" >&2
    exit 1
fi
[[ "$(wc -l <"$ATTEMPTS" | tr -d '[:space:]')" == "1" ]]
grep -q 'invalid stream-json' "$TEST_ROOT/success-invalid.err"

: >"$ATTEMPTS"
run_failover "$PROMPT_FILE" >"$TEST_ROOT/catalog.out" 2>"$TEST_ROOT/catalog.err"
[[ "$(paste -sd, "$ATTEMPTS")" == "first,second" ]]

awk 'BEGIN { for (i = 0; i < 9001; i++) printf "x" }' >"$TEST_ROOT/large.txt"
if run_failover "$TEST_ROOT/large.txt" first >"$TEST_ROOT/large.out" 2>"$TEST_ROOT/large.err"; then
    echo "oversized prompt unexpectedly succeeded" >&2
    exit 1
fi
grep -q 'failover prompt limit is 9000 bytes' "$TEST_ROOT/large.err"

echo "provider failover self-test passed"
