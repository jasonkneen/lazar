#!/usr/bin/env bash
set -uo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
EVAL="$LAZAR_HOME/workspace/evals/model-evals.sh"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

cat > "$T/genuine.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-1","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"tool_result","tool_use_id":"call-1","requested_command":"printf 'EVAL_TOOL_OK'","executed_command":"printf 'EVAL_TOOL_OK'","content":"EVAL_TOOL_OK\n[exit 0]"}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL

if ! bash "$EVAL" --analyze-jsonl EVAL_TOOL_OK < "$T/genuine.jsonl"; then
  echo "FAIL: a genuine matching tool_use/tool_result sequence must pass" >&2
  exit 1
fi

echo "PASS: genuine tool sequence"

cat > "$T/narrated.jsonl" <<'JSONL'
{"type":"text_delta","text":"I ran printf 'EVAL_TOOL_OK' and it printed EVAL_TOOL_OK."}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL

if bash "$EVAL" --analyze-jsonl EVAL_TOOL_OK < "$T/narrated.jsonl"; then
  echo "FAIL: narrated tool use must not pass" >&2
  exit 1
fi

echo "PASS: narrated tool use rejected"

cat > "$T/failed-verify.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-2","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"tool_result","tool_use_id":"call-2","content":"EVAL_TOOL_OK\n[exit 0]"}
{"type":"verify_report","report":{"all_passed":false,"any_failed":true,"failed_count":1}}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL

if bash "$EVAL" --analyze-jsonl EVAL_TOOL_OK < "$T/failed-verify.jsonl"; then
  echo "FAIL: a failed kernel verification must fail the model eval" >&2
  exit 1
fi

echo "PASS: failed verification rejected"

cat > "$T/fabrication.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-3","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"tool_result","tool_use_id":"call-3","content":"EVAL_TOOL_OK\n[exit 0]"}
{"type":"fabrication_check","flagged":true,"tool_uses_in_turn":1}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL

if bash "$EVAL" --analyze-jsonl EVAL_TOOL_OK < "$T/fabrication.jsonl"; then
  echo "FAIL: a kernel fabrication warning must fail the model eval" >&2
  exit 1
fi

echo "PASS: fabrication warning rejected"

if bash "$EVAL" --analyze-jsonl EVAL_TOOL_OK --require-verify < "$T/genuine.jsonl"; then
  echo "FAIL: --require-verify must reject a stream with no verify_report" >&2
  exit 1
fi

echo "PASS: missing required verification rejected"

cat > "$T/workflow.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-read","name":"bash","input":{"command":"cat config.txt"}}
{"type":"tool_result","tool_use_id":"call-read","content":"LIMIT=16384\n[exit 0]"}
{"type":"tool_use","id":"call-edit","name":"bash","input":{"command":"sed -i.bak 's/16384/128000/' config.txt && ./verify.sh"}}
{"type":"tool_result","tool_use_id":"call-edit","content":"WORKFLOW_GROUNDED_OK\n[exit 0]"}
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL

if ! bash "$EVAL" --analyze-workflow-jsonl WORKFLOW_GROUNDED_OK < "$T/workflow.jsonl"; then
  echo "FAIL: a grounded multi-tool workflow must pass" >&2
  exit 1
fi

echo "PASS: grounded multi-tool workflow"

cat > "$T/failed-command.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-4","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'; exit 1"}}
{"type":"tool_result","tool_use_id":"call-4","content":"EVAL_TOOL_OK\n[exit 1]"}
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL

if bash "$EVAL" --analyze-jsonl EVAL_TOOL_OK --require-verify < "$T/failed-command.jsonl"; then
  echo "FAIL: a failed executed command must fail the model eval" >&2
  exit 1
fi

echo "PASS: failed command rejected"
