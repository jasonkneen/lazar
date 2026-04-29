#!/usr/bin/env bash
# lazar-chat — minimal interactive REPL for lazar.
# No dependencies beyond bash. Pipes assistant text through `glow` for
# markdown rendering when available; falls back to raw text otherwise.
#
# This is the "I just want to chat" wrapper. For something richer, ask:
#   lazar -p "build me a TUI"
# and read _meta/create-tui/SKILL.md.
set -uo pipefail

if ! command -v lazar >/dev/null 2>&1; then
    echo "error: lazar not found in PATH. Run setup.sh first." >&2
    exit 1
fi

HAS_GLOW=0
command -v glow >/dev/null 2>&1 && HAS_GLOW=1

DIM=$'\033[2m'
BOLD=$'\033[1m'
CYAN=$'\033[36m'
GRAY=$'\033[90m'
RESET=$'\033[0m'

WIDTH=$(tput cols 2>/dev/null || echo 60)
[ "$WIDTH" -gt 60 ] && WIDTH=60
LINE=$(printf '%*s' "$WIDTH" '' | tr ' ' '─')

echo
echo "${GRAY}${LINE}${RESET}"
echo "  ${BOLD}lazar chat${RESET}  ${DIM}— Enter to send, 'exit' or Ctrl-C to quit${RESET}"
if [ "$HAS_GLOW" -eq 0 ]; then
    echo "  ${DIM}(install 'glow' for markdown rendering: brew install glow)${RESET}"
fi
echo "${GRAY}${LINE}${RESET}"
echo

while IFS= read -e -r -p "${CYAN}▶${RESET} " prompt; do
    case "$prompt" in
        "") continue ;;
        exit|quit|:q) break ;;
    esac

    echo
    if [ "$HAS_GLOW" -eq 1 ]; then
        # glow renders the assistant's markdown. lazar's text mode already
        # streams live; glow buffers and re-renders, so we trade liveness
        # for prettiness. For live + pretty, use a TUI (see create-tui).
        lazar -p "$prompt" 2>/dev/null | glow -
    else
        lazar -p "$prompt"
    fi
    echo
done

echo
echo "${DIM}bye.${RESET}"
