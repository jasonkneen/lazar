#!/usr/bin/env bash
# example-foreign-agent.sh — reference adapter target.
#
# Demonstrates the MINIMAL contract any agent must satisfy to be scored by
# harness-evals: execute the prompt and emit NDJSON events to stdout.
#
# Emit these event types (tool events as applicable, terminal event required):
#   {"type":"tool_use","name":"bash","input":{"command":"..."}}
#   {"type":"tool_result","tool_use_id":"...","content":"...","exit_code":0}
#   {"type":"text_delta","text":"..."}
#   {"type":"invoke_end","stop_reason":"end_turn"}
#
# This mock is deliberately dumb: it extracts a shell command from the prompt,
# runs it for real, and reports honestly. Replace with your own agent.
set -uo pipefail
PROMPT="${1:-}"

emit(){ printf '%s\n' "$1"; }
json_escape(){ python3 -c 'import json,sys; print(json.dumps(sys.stdin.read())[1:-1])'; }

emit '{"type":"invoke_start","model":"example-foreign-agent"}'

# Naive: run the first `echo ...` found in the prompt.
CMD=$(printf '%s' "$PROMPT" | rg -o 'echo [A-Za-z0-9_]+' | head -1)

if [[ -n "$CMD" ]]; then
  OUT=$(bash -c "$CMD" 2>&1); RC=$?
  ESC=$(printf '%s' "$OUT" | json_escape)
  CESC=$(printf '%s' "$CMD" | json_escape)
  emit "{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"bash\",\"input\":{\"command\":\"$CESC\"}}"
  emit "{\"type\":\"tool_result\",\"tool_use_id\":\"t1\",\"content\":\"$ESC\",\"exit_code\":$RC}"
  emit "{\"type\":\"text_delta\",\"text\":\"Ran the command.\"}"
else
  emit '{"type":"text_delta","text":"Hello! No command requested."}'
fi

emit '{"type":"invoke_end","stop_reason":"end_turn"}'
