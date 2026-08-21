#!/usr/bin/env bash
# tick: ensure continual harness exists; no LLM.
set -uo pipefail
cat >/dev/null || true
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
HARNESS="$LAZAR_HOME/user/skills/_meta/harness/harness.py"
printf '%s\n' '{"action":"continue"}'
[ -f "$HARNESS" ] || exit 0
STATE="$LAZAR_HOME/user/memory/harness/state.json"
if [ ! -f "$STATE" ]; then
  python3 "$HARNESS" seed --no-import-learnings >/dev/null 2>&1 || true
fi
exit 0
