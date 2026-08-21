#!/usr/bin/env bash
# lazar-guardrails.sh — one switch for ALL of lazar's enforcement layers.
#
#   lazar-guardrails off      # full operator access (no sandbox, unlocked kernel)
#   lazar-guardrails on       # restore enforcement
#   lazar-guardrails status   # show current state
#
# The switch only turns sandbox-exec on or off for the next process.
# Kernel and TUI source stay writable in either mode. Missing state means off.
set -euo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

case "$LAZAR_HOME" in
    ""|"/"|"$HOME") echo "error: unsafe LAZAR_HOME: $LAZAR_HOME" >&2; exit 1 ;;
    /*) ;;
    *) echo "error: LAZAR_HOME must be absolute: $LAZAR_HOME" >&2; exit 1 ;;
esac
LAZAR_HOME=$(cd "$LAZAR_HOME" 2>/dev/null && pwd -P) || {
    echo "error: cannot resolve LAZAR_HOME" >&2
    exit 1
}
STATE_FILE="$LAZAR_HOME/user/.guardrails"
if [[ ! -e "$STATE_FILE" && -f "$LAZAR_HOME/.guardrails" ]]; then
    STATE_FILE="$LAZAR_HOME/.guardrails"
fi
HOOKS="$LAZAR_HOME/user/hooks/pre-tool.d"
if [[ ! -d "$HOOKS" && -d "$LAZAR_HOME/user/hooks/pre-tool.d" ]]; then
    HOOKS="$LAZAR_HOME/user/hooks/pre-tool.d"
fi
MODE_TMP=""

cleanup() {
    if [[ -n "$MODE_TMP" && -e "$MODE_TMP" ]]; then
        rm -f -- "$MODE_TMP"
    fi
}
trap cleanup EXIT

prepare_mode() {
    local mode="$1"
    if [[ -e "$STATE_FILE" || -L "$STATE_FILE" ]]; then
        [[ ! -L "$STATE_FILE" && -f "$STATE_FILE" ]] || {
            echo "error: $STATE_FILE must be a regular file" >&2
            return 1
        }
    fi
    [[ -z "$MODE_TMP" ]] || rm -f -- "$MODE_TMP"
    umask 077
    MODE_TMP=$(mktemp "$LAZAR_HOME/.guardrails.XXXXXX")
    printf '%s\n' "$mode" >"$MODE_TMP"
    chmod 600 "$MODE_TMP"
}

commit_mode() {
    [[ -n "$MODE_TMP" && -f "$MODE_TMP" && ! -L "$MODE_TMP" ]] || return 1
    mv -f "$MODE_TMP" "$STATE_FILE"
    MODE_TMP=""
}

discard_mode() {
    if [[ -n "$MODE_TMP" ]]; then
        rm -f -- "$MODE_TMP"
        MODE_TMP=""
    fi
}

write_mode() {
    prepare_mode "$1"
    commit_mode
}

read_mode() {
    if [[ ! -e "$STATE_FILE" ]]; then
        printf '%s' "on"
        return
    fi
    [[ ! -L "$STATE_FILE" && -f "$STATE_FILE" ]] || {
        echo "error: $STATE_FILE must be a regular file" >&2
        return 1
    }
    [[ "$(wc -c <"$STATE_FILE" | tr -d '[:space:]')" -le 16 ]] || {
        echo "error: $STATE_FILE is oversized" >&2
        return 1
    }
    local mode
    mode=$(sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$STATE_FILE")
    case "$mode" in
        on|off) printf '%s' "$mode" ;;
        *) echo "error: invalid guardrails mode '$mode' in $STATE_FILE" >&2; return 1 ;;
    esac
}

guardrails_off() {
    echo "[guardrails] DISABLING enforcement"

    [[ -f "$LAZAR_HOME/bin/lazar" ]] || { echo "error: missing bin/lazar" >&2; return 1; }
    [[ -d "$LAZAR_HOME/src" ]] || { echo "error: missing src/" >&2; return 1; }
    read_mode >/dev/null

    # Prove that this caller can write at the protected Lazar root before
    # changing any other layer. Keep the new value in a private temporary file
    # until every layer is disabled, so an interrupted transition still reads
    # as sandboxed.
    prepare_mode off

    # Disable active veto hooks with a dedicated suffix. Operator-disabled
    # *.disabled hooks are left alone and will never be resurrected by "on".
    if [ -d "$HOOKS" ]; then
        for f in "$HOOKS"/*.sh; do
            [ -e "$f" ] || continue
            if ! mv "$f" "$f.guardrails-disabled"; then
                echo "error: could not disable hook $(basename "$f"); restoring guardrails" >&2
                guardrails_on || true
                return 1
            fi
            echo "  - hook disabled: $(basename "$f")"
        done
    fi

    if ! commit_mode; then
        echo "error: could not publish guardrails-off state; restoring enforcement" >&2
        guardrails_on || true
        return 1
    fi
    echo "  - sandbox-exec: OFF for the next Lazar process"
    echo "[guardrails] OFF — proposal/test/rollback still available, just not enforced."
}

guardrails_on() {
    echo "[guardrails] RESTORING enforcement"

    [[ -f "$LAZAR_HOME/bin/lazar" ]] || { echo "error: missing bin/lazar" >&2; return 1; }
    [[ -d "$LAZAR_HOME/src" ]] || { echo "error: missing src/" >&2; return 1; }
    # Publish the fail-closed state first. If locking another layer is
    # interrupted, every newly started Lazar process still uses the sandbox.
    write_mode on
    echo "  - sandbox-exec: ON for the next Lazar process"
    echo "  - src/ and tui/: left writable"

    if [ -d "$HOOKS" ]; then
        for f in "$HOOKS"/*.guardrails-disabled; do
            [ -e "$f" ] || continue
            mv "$f" "${f%.guardrails-disabled}"
            echo "  - hook re-enabled: $(basename "${f%.guardrails-disabled}")"
        done
    fi

    echo "[guardrails] ON"
}

guardrails_status() {
    echo "lazar guardrails status"
    echo "───────────────────────"
    local declared; declared=$(read_mode)
    echo "declared:        $declared"

    if [ "$declared" = "off" ]; then
        echo "sandbox:         OFF (full operator access)"
    else
        echo "sandbox:         ON"
    fi

    if [ -w "$LAZAR_HOME/src/src/main.rs" ]; then
        echo "src/:            writable"
    else
        echo "src/:            not writable"
    fi

    local active=0 toggled_off=0 operator_disabled=0
    if [[ -d "$HOOKS" ]]; then
        active=$(find "$HOOKS" -maxdepth 1 -type f -name '*.sh' | wc -l | tr -d ' ')
        toggled_off=$(find "$HOOKS" -maxdepth 1 -type f -name '*.guardrails-disabled' | wc -l | tr -d ' ')
        operator_disabled=$(find "$HOOKS" -maxdepth 1 -type f -name '*.disabled' | wc -l | tr -d ' ')
    fi
    echo "pre-tool hooks:  $active active, $toggled_off guardrails-disabled, $operator_disabled operator-disabled"
}

case "${1:-status}" in
    off|disable) guardrails_off ;;
    on|enable)   guardrails_on ;;
    status)      guardrails_status ;;
    *) echo "usage: lazar-guardrails [off|on|status]" >&2; exit 2 ;;
esac
