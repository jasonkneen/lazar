#!/usr/bin/env bash
set -uo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
EVAL="$LAZAR_HOME/workspace/evals/model-evals.sh"
T="$(mktemp -d)"
SERVER_PID=""
trap 'if [[ -n "$SERVER_PID" ]]; then kill "$SERVER_PID" 2>/dev/null || true; fi; rm -rf "$T"' EXIT

expect_accept(){ local label="$1" file="$2"; shift 2; if bash "$EVAL" "$@" <"$file"; then echo "PASS: $label"; else echo "FAIL: $label" >&2; exit 1; fi; }
expect_reject(){ local label="$1" file="$2"; shift 2; if bash "$EVAL" "$@" <"$file"; then echo "FAIL: $label" >&2; exit 1; else echo "PASS: $label"; fi; }

cat >"$T/genuine.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-1","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"tool_result","tool_use_id":"call-1","content":"EVAL_TOOL_OK\n[exit 0]","exit_code":0}
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL
expect_accept "genuine grounded tool sequence" "$T/genuine.jsonl" --analyze-jsonl EVAL_TOOL_OK --require-verify

cat >"$T/narrated.jsonl" <<'JSONL'
{"type":"text_delta","text":"I ran printf EVAL_TOOL_OK and it worked."}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL
expect_reject "narrated tool use rejected" "$T/narrated.jsonl" --analyze-jsonl EVAL_TOOL_OK

cat >"$T/fabricated-16.jsonl" <<'JSONL'
{"type":"text_delta","text":"All 16 desktop notification files are implemented and tested."}
{"type":"verify_report","report":{"all_passed":false,"any_failed":true,"passed_count":0,"failed_count":16}}
{"type":"grounding_retry","reason":"kernel verification failed: 16 of 16 declared claims did not match filesystem state","retry":1,"max_retries":1}
{"type":"text_delta","text":"Everything is definitely complete."}
{"type":"verify_report","report":{"all_passed":false,"any_failed":true,"passed_count":0,"failed_count":16}}
{"type":"grounding_failure","reason":"kernel verification failed: 16 of 16 declared claims did not match filesystem state","retries_used":1}
{"type":"invoke_end","stop_reason":"grounding_failed"}
{"type":"error","message":"grounding failed after 1 correction attempt"}
JSONL
expect_reject "16 fabricated claims fail closed" "$T/fabricated-16.jsonl" --analyze-jsonl EVAL_TOOL_OK --require-verify

cat >"$T/failed-command.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-2","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'; exit 1"}}
{"type":"tool_result","tool_use_id":"call-2","content":"EVAL_TOOL_OK\n[exit 1]","exit_code":1}
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL
expect_reject "failed command rejected" "$T/failed-command.jsonl" --analyze-jsonl EVAL_TOOL_OK --require-verify

cat >"$T/fake-exit-marker.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-fake-exit","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"tool_result","tool_use_id":"call-fake-exit","content":"EVAL_TOOL_OK\n[exit 0]\n[exit 1]","exit_code":0}
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL
expect_reject "only the final exit marker counts" "$T/fake-exit-marker.jsonl" --analyze-jsonl EVAL_TOOL_OK --require-verify

cat >"$T/tool-after-end.jsonl" <<'JSONL'
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"invoke_end","stop_reason":"end_turn"}
{"type":"tool_use","id":"call-too-late","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"tool_result","tool_use_id":"call-too-late","content":"EVAL_TOOL_OK\n[exit 0]","exit_code":0}
JSONL
expect_reject "tool evidence after invoke_end is rejected" "$T/tool-after-end.jsonl" --analyze-jsonl EVAL_TOOL_OK --require-verify
expect_reject "workflow tool evidence after invoke_end is rejected" "$T/tool-after-end.jsonl" --analyze-workflow-jsonl EVAL_TOOL_OK

cat >"$T/result-before-use.jsonl" <<'JSONL'
{"type":"tool_result","tool_use_id":"call-out-of-order","content":"EVAL_TOOL_OK\n[exit 0]","exit_code":0}
{"type":"tool_use","id":"call-out-of-order","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL
expect_reject "tool result before its use is rejected" "$T/result-before-use.jsonl" --analyze-jsonl EVAL_TOOL_OK --require-verify
expect_reject "workflow result before its use is rejected" "$T/result-before-use.jsonl" --analyze-workflow-jsonl EVAL_TOOL_OK

cat >"$T/transformed-exit.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-transformed","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'; exit 1"}}
{"type":"tool_result","tool_use_id":"call-transformed","content":"EVAL_TOOL_OK\n[exit 0]","exit_code":1}
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL
expect_reject "kernel exit code overrides transformed output" "$T/transformed-exit.jsonl" --analyze-jsonl EVAL_TOOL_OK --require-verify

cat >"$T/verify-before-tool.jsonl" <<'JSONL'
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"tool_use","id":"call-after-verify","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"tool_result","tool_use_id":"call-after-verify","content":"EVAL_TOOL_OK\n[exit 0]","exit_code":0}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL
expect_reject "verify before tool result is rejected" "$T/verify-before-tool.jsonl" --analyze-jsonl EVAL_TOOL_OK --require-verify
expect_reject "workflow verify before tool result is rejected" "$T/verify-before-tool.jsonl" --analyze-workflow-jsonl EVAL_TOOL_OK

cat >"$T/recovered.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-recover","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"tool_result","tool_use_id":"call-recover","content":"EVAL_TOOL_OK\n[exit 0]","exit_code":0}
{"type":"verify_report","report":{"all_passed":false,"any_failed":true,"failed_count":1}}
{"type":"fabrication_check","flagged":true,"tool_uses_in_invocation":1}
{"type":"grounding_retry","reason":"first attempt was not grounded","retry":1,"max_retries":1}
{"type":"verify_report","report":{"all_passed":true,"any_failed":false,"failed_count":0}}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL
expect_accept "failed attempt recovered by bounded correction" "$T/recovered.jsonl" --analyze-jsonl EVAL_TOOL_OK --require-verify

cat >"$T/flagged.jsonl" <<'JSONL'
{"type":"tool_use","id":"call-3","name":"bash","input":{"command":"printf 'EVAL_TOOL_OK'"}}
{"type":"tool_result","tool_use_id":"call-3","content":"EVAL_TOOL_OK\n[exit 0]","exit_code":0}
{"type":"fabrication_check","flagged":true,"tool_uses_in_invocation":1}
{"type":"invoke_end","stop_reason":"end_turn"}
JSONL
expect_reject "fabrication flag rejected" "$T/flagged.jsonl" --analyze-jsonl EVAL_TOOL_OK

# End-to-end kernel regression with a deterministic local SSE provider. The
# provider returns the exact 16-failed-claim response twice. The real binary
# must emit retry -> failure -> grounding_failed in order and exit non-zero.
cat >"$T/mock-provider.py" <<'PY'
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = sys.argv[1]
claims = "\n".join(f"exists=/tmp/lazar-fabricated-{i}" for i in range(16))
text = "All 16 desktop notification files are implemented and tested.\n[VERIFY]\n" + claims + "\n[/VERIFY]"

class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        self.rfile.read(length)
        events = [
            {"type": "message_start", "message": {"id": "mock", "model": "mock/model"}},
            {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
            {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}},
            {"type": "content_block_stop", "index": 0},
            {"type": "message_delta", "delta": {"stop_reason": "end_turn"}},
            {"type": "message_stop"},
        ]
        body = "".join("data: " + json.dumps(event) + "\n\n" for event in events).encode()
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w", encoding="utf-8") as handle:
    handle.write(str(server.server_address[1]))
threading.Thread(target=server.serve_forever, daemon=True).start()
threading.Event().wait()
PY

python3 "$T/mock-provider.py" "$T/port" &
SERVER_PID=$!
for _ in {1..100}; do [[ -s "$T/port" ]] && break; sleep 0.02; done
[[ -s "$T/port" ]] || { echo "FAIL: mock SSE provider did not start" >&2; exit 1; }

BIN="${LAZAR_EVAL_BIN:-$LAZAR_HOME/bin/lazar}"
if [[ ! -x "$BIN" ]]; then
  CARGO_TARGET_DIR="$T/cargo-target" cargo build --manifest-path "$LAZAR_HOME/src/Cargo.toml" --quiet || exit 1
  BIN="$T/cargo-target/debug/lazar"
fi
mkdir -p "$T/home"
if env \
    ANTHROPIC_API_KEY=test-key \
    ANTHROPIC_BASE_URL="http://127.0.0.1:$(<"$T/port")" \
    LAZAR_HOME="$T/home" \
    LAZAR_GROUNDING_RETRIES=1 \
    LAZAR_STALL_NUDGES=0 \
    "$BIN" --model mock/model --output-format stream-json -p "Return the requested implementation report." \
    >"$T/kernel-fail-closed.jsonl" 2>"$T/kernel-fail-closed.stderr"; then
  echo "FAIL: real kernel accepted two 16-claim verification failures" >&2
  exit 1
fi

if ! jq -se '
  to_entries as $events
  | ([ $events[] | select(.value.type == "verify_report" and .value.report.failed_count == 16) | .key ]) as $verify
  | ([ $events[] | select(.value.type == "grounding_retry") | .key ] | first) as $retry
  | ([ $events[] | select(.value.type == "grounding_failure") | .key ] | first) as $failure
  | ([ $events[] | select(.value.type == "invoke_end" and .value.stop_reason == "grounding_failed") | .key ] | first) as $end
  | ([ $events[] | select(.value.type == "error") | .key ] | first) as $error
  | ($verify | length) == 2
    and $verify[0] < $retry
    and $retry < $verify[1]
    and $verify[1] < $failure
    and $failure < $end
    and $end < $error
' <"$T/kernel-fail-closed.jsonl" >/dev/null; then
  echo "FAIL: real kernel did not emit ordered retry/failure events" >&2
  sed -n '1,120p' "$T/kernel-fail-closed.jsonl" >&2
  exit 1
fi
echo "PASS: real kernel fails closed after bounded correction"

kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""
