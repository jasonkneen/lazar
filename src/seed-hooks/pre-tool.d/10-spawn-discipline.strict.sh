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
COMMAND=$(printf '%s' "$PAYLOAD" | jq -r '.command // ""' 2>/dev/null)
[ -z "$COMMAND" ] && COMMAND=$(printf '%s' "$PAYLOAD" | sed -n 's/.*"command":"\(.*\)".*/\1/p')

# Allowlist: the sanctioned wrapper.
if printf '%s' "$COMMAND" | rg -q 'subagent/spawn\.sh'; then
  echo '{"action":"continue"}'
  exit 0
fi

# Veto any invocation of the lazar binary with -p (a raw sub-agent spawn).
if printf '%s' "$COMMAND" | rg -q '(^|[[:space:]]|/)lazar[[:space:]]+([^[:space:]]+[[:space:]]+)*-p([[:space:]]|=|$)'; then
  cat <<'EOF'
{"action":"veto","reason":"raw `lazar -p` spawns are disabled — delegate via the subagent core skill: bash skills/_meta/subagent/spawn.sh <task-id> <prompt-file>"}
EOF
  exit 0
fi

echo '{"action":"continue"}'
