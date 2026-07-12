#!/usr/bin/env bash
# pre-tool (STRICT / fail-closed): raw `lazar -p` spawns are disabled.
# Delegation must go through the locked binary: $LAZAR_HOME/bin/lazar-spawn
# (skills/_meta/subagent/spawn.sh is only a thin re-exec to that binary).
#
# Model-agnostic: same rule at every depth for every provider. The wrapper's
# internal `lazar -p` runs inside the tool process (not as tool_use), so this
# hook never sees it — no false positive.
#
# .strict. => fail-closed: hook errors / bad JSON become vetoes.
PAYLOAD=$(cat)
COMMAND=$(printf '%s' "$PAYLOAD" | jq -r '.command // empty' 2>/dev/null)
if [ -z "$COMMAND" ]; then
  cat <<'EOF'
{"action":"veto","reason":"spawn-discipline: could not parse pre-tool command JSON"}
EOF
  exit 0
fi

# Raw `lazar -p` / `lazar --prompt` is always vetoed. Do NOT short-circuit on
# a spawn path substring first — that was a bypass ("echo lazar-spawn; lazar -p …").
if printf '%s' "$COMMAND" | rg -q '(^|[[:space:]/;])lazar([[:space:]]+[^[:space:]]+)*[[:space:]]+(-p|--prompt)([[:space:]]|=|$)'; then
  cat <<'EOF'
{"action":"veto","reason":"raw `lazar -p` spawns are disabled — delegate via locked bin: bash $LAZAR_HOME/bin/lazar-spawn <task-id> <prompt-file> (or skills/_meta/subagent/spawn.sh which re-execs it)"}
EOF
  exit 0
fi

echo '{"action":"continue"}'
