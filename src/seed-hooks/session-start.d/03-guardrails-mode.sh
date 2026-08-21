#!/usr/bin/env bash
# session-start: inject the operator-selected enforcement mode as valid hook JSON.
set -uo pipefail

# The kernel sends a JSON payload even though this hook does not need it.
if command -v jq >/dev/null 2>&1; then
  jq -c . >/dev/null 2>&1 || cat >/dev/null
else
  cat >/dev/null
fi

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
STATE=$(cat "$LAZAR_HOME/.guardrails" 2>/dev/null || echo on)

if [ "$STATE" = "off" ]; then
  CONTEXT='GUARDRAILS: OFF — normal operator filesystem access. Follow the requested scope carefully; do not apply sandbox-only write restrictions.'
else
  CONTEXT='GUARDRAILS: ON — writes are limited to skills/, memory/, workspace/, /tmp, approved directories, and documented session/auth paths. logs/, bin/, and src/ are protected.'
fi

if command -v jq >/dev/null 2>&1; then
  jq -cn --arg ctx "$CONTEXT" '{action:"inject", context:$ctx}'
else
  python3 -c 'import json,sys; print(json.dumps({"action":"inject","context":sys.stdin.read().strip()}))' <<<"$CONTEXT"
fi
