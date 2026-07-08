#!/usr/bin/env bash
# pre-tool (STRICT / fail-closed): raw `lazar -p` spawns are disabled.
# Delegation must go through the subagent core skill's spawn.sh wrapper.
#
# Fires at EVERY depth, so the discipline enforces recursively — a sub-agent
# cannot hand-roll a raw spawn either (context injection can't reach children;
# this can). The wrapper's own internal `lazar -p` exec runs inside the tool
# process, not as a tool_use, so the hook never sees it — no false positive.
#
# .strict. => fail-closed: if this hook itself errors, the kernel treats it as
# a veto, so spawning is blocked rather than silently allowed.
PAYLOAD=$(cat)
COMMAND=$(printf '%s' "$PAYLOAD" | jq -r '.command // empty' 2>/dev/null)
if [ -z "$COMMAND" ]; then
  # Fail closed on unparseable payload — never fall back to greedy sed.
  cat <<'EOF'
{"action":"veto","reason":"spawn-discipline: could not parse pre-tool command JSON"}
EOF
  exit 0
fi

# Raw `lazar -p` / `lazar --prompt` is always vetoed. Do NOT short-circuit on
# a spawn.sh substring first — that was a bypass ("echo spawn.sh; lazar -p …").
if printf '%s' "$COMMAND" | rg -q '(^|[[:space:]/;])lazar([[:space:]]+[^[:space:]]+)*[[:space:]]+(-p|--prompt)([[:space:]]|=|$)'; then
  cat <<'EOF'
{"action":"veto","reason":"raw `lazar -p` spawns are disabled — delegate via the subagent core skill: bash skills/_meta/subagent/spawn.sh <task-id> <prompt-file>"}
EOF
  exit 0
fi

echo '{"action":"continue"}'
