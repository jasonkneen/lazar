#!/usr/bin/env bash
# session-start: inject a lightweight "recent activity" digest so a fresh
# session isn't amnesiac. Every TUI launch mints a NEW session (no history
# replay — deliberate), which left lazar unable to answer "the design?" /
# "what we worked on recently". This injects the last few REAL user prompts
# from the shared stream log — the cheapest, highest-signal recall, no LLM.
#
# Bounded: reads only the tail of stream.jsonl. Emits inject JSON or continue.
cat > /dev/null  # drain payload

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
LOG="$LAZAR_HOME/logs/stream.jsonl"
[ -f "$LOG" ] || { echo '{"action":"continue"}'; exit 0; }
command -v jq >/dev/null 2>&1 || { echo '{"action":"continue"}'; exit 0; }

# Last N lines is plenty for recent prompts; never cat the whole file.
# `-R 'fromjson?'` tolerates partial/malformed lines (one bad line won't abort).
RECENT=$(tail -n 400 "$LOG" 2>/dev/null \
  | jq -R -rc 'fromjson? | select(.kind=="user") | .content' 2>/dev/null \
  | grep -v '^You said you would' \
  | grep -v '^Continue\. Use the bash' \
  | grep -v '^Your turn ended incomplete' \
  | grep -v '^run heartbeat' \
  | grep -v '^Read this state and add' \
  | grep -v '^add one line to' \
  | awk 'NF' \
  | tail -n 8)

[ -z "$RECENT" ] && { echo '{"action":"continue"}'; exit 0; }

# Trim each line to one bounded sentence for the digest.
DIGEST=$(printf '%s\n' "$RECENT" | while IFS= read -r line; do
  printf -- '- %.140s\n' "$line"
done)

CONTEXT=$(printf 'RECENT ACTIVITY (last things the operator asked — this is a fresh session, so use these to resolve "the X we discussed"):\n%s\n\nIf a prompt refers to earlier work you do not see here, read memory/PINNED.md and bounded logs before asking "which one?".' "$DIGEST")

jq -n --arg c "$CONTEXT" '{action:"inject", context:$c}'
