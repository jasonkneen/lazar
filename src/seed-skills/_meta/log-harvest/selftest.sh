#!/usr/bin/env bash
# selftest for log-harvest — runs against a synthetic log in a temp LAZAR_HOME.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0; FAIL=0
ok(){ echo "[PASS] $1"; PASS=$((PASS+1)); }
no(){ echo "[FAIL] $1"; FAIL=$((FAIL+1)); }
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
mkdir -p "$T/logs" "$T/memory/distilled"

# Synthetic log: a real prompt, a file-writing command, and a distilled file that must survive.
cat > "$T/logs/stream.jsonl" <<EOF
{"kind":"user","content":"build the pricing page","ts_ms":1783000000000}
{"kind":"tool_result","command":"cat > /Users/x/lazar/workspace/pricing/index.html <<E","ts_ms":1783000001000}
{"kind":"user","content":"You said you would do something","ts_ms":1783000002000}
EOF
echo "keep me" > "$T/memory/distilled/stream.jsonl.999.bak.md"

LAZAR_HOME="$T" bash "$HERE/harvest.sh" >/dev/null 2>&1

J="$T/memory/work-journal.md"
[ -f "$J" ] && ok "journal created" || no "journal not created"
rg -q 'build the pricing page' "$J" 2>/dev/null && ok "real prompt captured" || no "prompt missing"
rg -q 'pricing/index.html' "$J" 2>/dev/null && ok "file-touched captured" || no "file path missing"
rg -q 'You said you would' "$J" 2>/dev/null && no "nudge leaked into journal" || ok "nudge correctly filtered"
[ -f "$T/memory/distilled/stream.jsonl.999.bak.md" ] && ok "distilled file untouched" || no "distilled file was deleted (whitelist broken)"

echo "---- $PASS passed, $FAIL failed ----"
[ "$FAIL" -eq 0 ]
