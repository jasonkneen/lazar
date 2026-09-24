#!/usr/bin/env bash
# lazar-failover — try provider-backed one-shot prompts without repeating side effects.
#
# A failed provider is eligible for failover only when its stream-json output is
# valid and contains no tool_use, tool_result, or hook_start event. Once a tool or hook event appears,
# the attempt may have changed the filesystem, so this wrapper stops immediately.
set -uo pipefail

usage() {
    cat <<'EOF'
usage: lazar-failover <prompt-file> [provider ...]

Run a cold, one-shot prompt through the listed provider IDs in order. When no
providers are listed, use key-available entries from `lazar providers --json`
in catalog order.

Failover is allowed only before the failed attempt emits a tool_use or
tool_result or hook_start event. Inherited session/model state is cleared.
EOF
}

fail() {
    echo "error: $*" >&2
    exit 1
}

case "${1:-}" in
    -h|--help)
        usage
        exit 0
        ;;
esac

[[ $# -ge 1 ]] || {
    usage >&2
    exit 2
}

command -v jq >/dev/null 2>&1 || fail "jq is required"

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
LAZAR_BIN="${LAZAR_BIN:-$LAZAR_HOME/bin/lazar}"
PROMPT_FILE="$1"
shift

[[ -x "$LAZAR_BIN" ]] || fail "lazar binary is not executable: $LAZAR_BIN"
[[ -f "$PROMPT_FILE" ]] || fail "prompt file not found: $PROMPT_FILE"

PROMPT_BYTES=$(wc -c <"$PROMPT_FILE" | tr -d '[:space:]')
[[ "$PROMPT_BYTES" -le 9000 ]] ||
    fail "prompt is $PROMPT_BYTES bytes; failover prompt limit is 9000 bytes"
PROMPT=$(<"$PROMPT_FILE")

PROVIDERS=()
if [[ $# -gt 0 ]]; then
    PROVIDERS=("$@")
else
    CATALOG_JSON=$("$LAZAR_BIN" providers --json) ||
        fail "could not read the provider catalog"
    while IFS= read -r provider; do
        [[ -n "$provider" ]] && PROVIDERS+=("$provider")
    done < <(printf '%s\n' "$CATALOG_JSON" |
        jq -er '.[] | select(.key_available == true) | .id')
fi

[[ ${#PROVIDERS[@]} -gt 0 ]] ||
    fail "no provider IDs supplied and no catalog provider has an available key"

# Validate and de-duplicate before making any request. Provider IDs are used as
# argv values, never shell code, but a narrow alphabet also makes diagnostics
# and fake-runner tests unambiguous.
ORDERED=()
SEEN="|"
for provider in "${PROVIDERS[@]}"; do
    [[ "$provider" =~ ^[A-Za-z0-9._-]+$ ]] ||
        fail "invalid provider ID: $provider"
    case "$SEEN" in
        *"|$provider|"*) continue ;;
    esac
    SEEN="${SEEN}${provider}|"
    ORDERED+=("$provider")
done

RUN_DIR=$(mktemp -d "${TMPDIR:-/tmp}/lazar-failover.XXXXXX") ||
    fail "could not create a temporary directory"
cleanup() {
    case "$RUN_DIR" in
        "${TMPDIR:-/tmp}"/lazar-failover.*|/tmp/lazar-failover.*|/private/tmp/lazar-failover.*)
            rm -rf -- "$RUN_DIR"
            ;;
        *)
            echo "refusing unsafe cleanup target: $RUN_DIR" >&2
            ;;
    esac
}
trap cleanup EXIT

normalize_stream() {
    local source="$1" destination="$2" line normalized saw=0
    : >"$destination"
    while IFS= read -r line || [[ -n "$line" ]]; do
        [[ ! "$line" =~ ^[[:space:]]*$ ]] || return 1
        normalized=$(printf '%s\n' "$line" |
            jq -cse 'if (length == 1 and (.[0] | type == "object")) then .[0] else error("expected exactly one JSON object") end' \
            2>/dev/null) || return 1
        printf '%s\n' "$normalized" >>"$destination"
        saw=1
    done <"$source"
    [[ $saw -eq 1 ]]
}

attempt=0
last_rc=1
for provider in "${ORDERED[@]}"; do
    attempt=$((attempt + 1))
    stdout_file="$RUN_DIR/$attempt.stdout.jsonl"
    stderr_file="$RUN_DIR/$attempt.stderr"
    normalized_file="$RUN_DIR/$attempt.normalized.jsonl"

    echo "[lazar-failover] attempt $attempt/${#ORDERED[@]}: $provider" >&2
    env -u LAZAR_SESSION_ID -u LAZAR_MODEL LAZAR_DEPTH=0 LAZAR_TRACE_HOOKS=1 "$LAZAR_BIN" --provider "$provider" --output-format stream-json -p "$PROMPT" \
        >"$stdout_file" 2>"$stderr_file"
    last_rc=$?

    cat "$stderr_file" >&2

    # Validate the advertised output contract even when the provider process
    # exits successfully. On failure, absence of tool evidence is unknowable;
    # surface the partial stream and stop instead of risking a duplicate
    # mutation on another provider.
    if ! normalize_stream "$stdout_file" "$normalized_file"; then
        cat "$stdout_file"
        echo "[lazar-failover] stopped: attempt emitted invalid stream-json" >&2
        [[ $last_rc -ne 0 ]] && exit "$last_rc"
        exit 1
    fi

    if [[ $last_rc -eq 0 ]]; then
        cat "$stdout_file"
        exit 0
    fi

    if jq -e 'select(.type == "tool_use" or .type == "tool_result" or .type == "hook_start")' \
        "$normalized_file" >/dev/null 2>&1; then
        cat "$stdout_file"
        echo "[lazar-failover] stopped: $provider emitted a tool or hook event; retry could duplicate side effects" >&2
        exit "$last_rc"
    fi

    echo "[lazar-failover] $provider failed before any tool event; trying the next provider" >&2
done

echo "[lazar-failover] all ${#ORDERED[@]} provider attempts failed before tool execution" >&2
exit "$last_rc"
