#!/usr/bin/env bash
# lazar-guardrails.sh — one switch for ALL of lazar's enforcement layers.
#
#   lazar-guardrails off      # full operator access (no sandbox, unlocked kernel)
#   lazar-guardrails on       # restore enforcement
#   lazar-guardrails status   # show current state
#
# WHAT THIS TOGGLES (4 independent layers):
#   1. sandbox-exec wrapper on every bash tool call   (LAZAR_NO_SANDBOX)
#   2. bin/lazar immutability                         (chflags uchg)
#   3. src/ read-only                                 (chmod a-w)
#   4. pre-tool veto hooks                            (hooks/pre-tool.d/*.sh)
#
# Turning guardrails OFF does not remove the proposal/test/rollback WORKFLOW —
# scripts/kernel-apply.sh, kernel-verify.sh and workspace/proposals/ all still
# work. It only stops the filesystem from *forcing* it.
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
STATE_FILE="$LAZAR_HOME/workspace/.guardrails"
HOOKS="$LAZAR_HOME/hooks/pre-tool.d"

case "$LAZAR_HOME" in
    ""|"/"|"$HOME") echo "error: unsafe LAZAR_HOME: $LAZAR_HOME" >&2; exit 1 ;;
esac

set_env_default() {
    # Rewrite the LAZAR_NO_SANDBOX default inside a launcher file.
    local file="$1" value="$2"
    [ -f "$file" ] || return 0
    if rg -q 'LAZAR_NO_SANDBOX:-[01]' "$file" 2>/dev/null; then
        sed -i '' "s/LAZAR_NO_SANDBOX:-[01]/LAZAR_NO_SANDBOX:-$value/g" "$file"
    fi
}

guardrails_off() {
    echo "[guardrails] DISABLING enforcement"

    # 1. sandbox off — launchers read workspace/.guardrails
    echo "off" > "$STATE_FILE"
    echo "  · sandbox-exec: OFF (launchers read workspace/.guardrails)"

    # 2 + 3. unlock kernel binary and source
    chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
    chmod u+w "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
    chmod -R u+w "$LAZAR_HOME/src" 2>/dev/null || true
    echo "  · bin/lazar: unlocked"
    echo "  · src/: writable"

    # 4. disable veto hooks (rename, don't delete — reversible)
    if [ -d "$HOOKS" ]; then
        for f in "$HOOKS"/*.sh; do
            [ -e "$f" ] || continue
            mv "$f" "$f.disabled" && echo "  · hook disabled: $(basename "$f")"
        done
    fi

    echo "off" > "$STATE_FILE"
    echo "[guardrails] OFF — proposal/test/rollback still available, just not enforced."
}

guardrails_on() {
    echo "[guardrails] RESTORING enforcement"

    echo "on" > "$STATE_FILE"
    echo "  · sandbox-exec: ON"

    chmod -R a-w "$LAZAR_HOME/src" 2>/dev/null || true
    chflags uchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
    echo "  · bin/lazar: locked (uchg)"
    echo "  · src/: read-only"

    if [ -d "$HOOKS" ]; then
        for f in "$HOOKS"/*.sh.disabled; do
            [ -e "$f" ] || continue
            # don't resurrect the shipped example
            case "$(basename "$f")" in
                00-example-*) continue ;;
            esac
            mv "$f" "${f%.disabled}" && echo "  · hook re-enabled: $(basename "${f%.disabled}")"
        done
    fi

    echo "on" > "$STATE_FILE"
    echo "[guardrails] ON"
}

guardrails_status() {
    echo "lazar guardrails status"
    echo "───────────────────────"
    local declared; declared=$(cat "$STATE_FILE" 2>/dev/null || echo "on (default)")
    echo "declared:        $declared"

    if [ "$declared" = "off" ]; then
        echo "sandbox:         OFF (full operator access)"
    else
        echo "sandbox:         ON"
    fi

    if ls -lO "$LAZAR_HOME/bin/lazar" 2>/dev/null | rg -q uchg; then
        echo "bin/lazar:       LOCKED (uchg)"
    else
        echo "bin/lazar:       unlocked"
    fi

    if [ -w "$LAZAR_HOME/src/src/main.rs" ]; then
        echo "src/:            writable"
    else
        echo "src/:            read-only"
    fi

    local active; active=$(ls "$HOOKS"/*.sh 2>/dev/null | wc -l | tr -d ' ')
    local off;    off=$(ls "$HOOKS"/*.disabled 2>/dev/null | wc -l | tr -d ' ')
    echo "pre-tool hooks:  $active active, $off disabled"
}

case "${1:-status}" in
    off|disable) guardrails_off ;;
    on|enable)   guardrails_on ;;
    status)      guardrails_status ;;
    *) echo "usage: lazar-guardrails [off|on|status]" >&2; exit 2 ;;
esac
