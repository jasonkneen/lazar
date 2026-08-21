#!/usr/bin/env bash
# session-start: inject compact continual-harness overview so every top-level
# invoke sees ranked durable lessons without the model opting into a skill.
# Fail-soft, bounded, no LLM.
set -uo pipefail
cat >/dev/null  # drain kernel payload

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
HARNESS_PY=""
for candidate in \
  "$LAZAR_HOME/user/skills/_meta/harness/harness.py" \
  "$LAZAR_HOME/src/seed-skills/_meta/harness/harness.py"
do
  if [ -f "$candidate" ]; then
    HARNESS_PY="$candidate"
    break
  fi
done

if [ -z "$HARNESS_PY" ]; then
  printf '%s\n' '{"action":"continue"}'
  exit 0
fi

# Ensure store exists (cheap; no-op if already seeded with entries unless empty)
STATE="$LAZAR_HOME/user/memory/harness/state.json"
if [ ! -f "$STATE" ]; then
  python3 "$HARNESS_PY" seed --no-import-learnings >/dev/null 2>&1 || true
fi

ARGS=(overview --limit 16)
if [ -n "${LAZAR_SESSION_ID:-}" ]; then
  ARGS=(--session "$LAZAR_SESSION_ID" overview --limit 16)
fi

TEXT="$(python3 "$HARNESS_PY" "${ARGS[@]}" 2>/dev/null || true)"
if [ -z "$TEXT" ]; then
  printf '%s\n' '{"action":"continue"}'
  exit 0
fi

# Cap inject size
TEXT="$(printf '%s' "$TEXT" | head -c 3500)"

if command -v jq >/dev/null 2>&1; then
  jq -n --arg c "$TEXT" '{action:"inject", context:$c}'
else
  python3 -c 'import json,sys; print(json.dumps({"action":"inject","context":sys.stdin.read()}))' <<<"$TEXT"
fi
