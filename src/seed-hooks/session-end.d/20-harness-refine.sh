#!/usr/bin/env bash
# session-end: capture 0–2 durable lessons into the continual harness.
# Fail-soft, no LLM required. Prefer structured gotchas over freeform dumps.
set -uo pipefail
PAYLOAD=$(cat || true)
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
HARNESS="$LAZAR_HOME/skills/_meta/harness/harness.py"
emit() { printf '%s\n' '{"action":"continue"}'; }

[ -f "$HARNESS" ] || { emit; exit 0; }

# Only clean top-level sessions
STATUS=$(printf '%s' "$PAYLOAD" | jq -r '.status // empty' 2>/dev/null || true)
DEPTH=$(printf '%s' "$PAYLOAD" | jq -r '.depth // 0' 2>/dev/null || echo 0)
[ "$DEPTH" = "0" ] || [ -z "$DEPTH" ] || { emit; exit 0; }

# Skip heartbeat-ish prompts
PROMPT=$(printf '%s' "$PAYLOAD" | jq -r '.prompt // empty' 2>/dev/null || true)
case "$PROMPT" in
  *heartbeat*|*run\ tick*|*--tick*) emit; exit 0 ;;
esac

LOG="$LAZAR_HOME/logs/stream.jsonl"
[ -f "$LOG" ] || { emit; exit 0; }

# Pull last assistant text + tool failures (bounded)
SNIP=$(tail -n 80 "$LOG" 2>/dev/null \
  | jq -R -rc 'fromjson? | select(.kind=="assistant" or .kind=="tool_result" or .kind=="grounding_retry" or .kind=="error") | [.kind, (.content // .note // .reason // .command // ""|tostring)] | @tsv' 2>/dev/null \
  | tail -n 20 || true)

# If grounding_retry appeared this session window, record a pitfall once
if printf '%s' "$SNIP" | rg -q 'grounding_retry|VERIFY'; then
  python3 "$HARNESS" add --kind memory \
    --title "verify-when-claiming-outcomes" \
    --content "When asserting file outcomes, always emit a valid [VERIFY]...[/VERIFY] block with exists/contains/size claims. Kernel rejects ungrounded completion language." \
    --evidence "session-end observed grounding_retry/VERIFY activity" >/dev/null 2>&1 || true
fi

# If tool_result shows non-zero exits repeatedly, note it
FAILS=$(printf '%s' "$SNIP" | rg -c $'\ttool_result' || true)
# Lightweight: if error kind present
if printf '%s' "$SNIP" | rg -q $'^error\t'; then
  python3 "$HARNESS" add --kind memory \
    --title "inspect-errors-before-retry" \
    --content "On tool/kernel errors, read the actual error line before retrying the same command. Empty commands and sandbox denials need a different approach, not a blind re-run." \
    --evidence "session-end saw error events in stream tail" >/dev/null 2>&1 || true
fi

emit
exit 0
