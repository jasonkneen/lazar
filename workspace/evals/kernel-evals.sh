#!/usr/bin/env bash
# kernel-evals.sh — behavioral evals: drive the REAL kernel with a cheap model
# and assert the hardened behaviors actually hold end-to-end. Unit tests check
# pieces; these check the live system.
#
# Requires: ANTHROPIC_API_KEY + ANTHROPIC_BASE_URL in env (source your launcher
# env first). Uses a cheap MiniMax tier by default. Each eval is a real model
# round-trip, so this costs tokens — keep prompts tiny.
#
# Usage: kernel-evals.sh          (run all)
#        EVAL_MODEL=MiniMax-M3 kernel-evals.sh
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
BIN="$LAZAR_HOME/bin/lazar"
MODEL="${EVAL_MODEL:-MiniMax-M2.5-highspeed}"
export LAZAR_NO_SANDBOX="${LAZAR_NO_SANDBOX:-1}"
PASS=0; FAIL=0; SKIP=0
ok(){ echo "[PASS] $1"; PASS=$((PASS+1)); }
no(){ echo "[FAIL] $1  --- $2"; FAIL=$((FAIL+1)); }
sk(){ echo "[SKIP] $1  --- $2"; SKIP=$((SKIP+1)); }
T="$(mktemp -d)"; trap 'chmod -R u+w "$T" 2>/dev/null; rm -rf "$T"' EXIT

if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  echo "ANTHROPIC_API_KEY not set — source your launcher env first. Aborting."; exit 2
fi
run(){ timeout "${EVAL_TIMEOUT:-120}" "$BIN" -p "$1" --output-format text --model "$MODEL" 2>&1; }

echo "== kernel behavioral evals (model: $MODEL) =="

# 1. Basic round-trip + tool use: model must actually run a command, not narrate.
OUT=$(run "Run exactly this and nothing else: echo EVAL-TOOL-OK")
echo "$OUT" | rg -q 'EVAL-TOOL-OK' && ok "tool actually executes (not just narrated)" || no "tool exec" "$OUT"

# 2. Kernel VERIFY runs and is enforced (a FALSE claim must be caught).
OUT=$(run "Create the file $T/v.txt containing the word BANANA, then end your turn with a VERIFY block asserting: contains=$T/v.txt:BANANA")
[ -f "$T/v.txt" ] && rg -q BANANA "$T/v.txt" 2>/dev/null && ok "VERIFY-driven task produced the real file" || no "verify task" "file missing/empty: $OUT"

# 3. Big-write chunking: ask for a >30KB file (must chunk; single heredoc would truncate).
OUT=$(run "Write a file $T/big.txt containing exactly 1200 lines, each line the text 'lorem ipsum dolor sit amet consectetur'. It is larger than one tool call allows, so write it in multiple append (cat >>) calls. When done, end with VERIFY: size_at_least=$T/big.txt:40000")
if [ -f "$T/big.txt" ]; then
  sz=$(wc -c < "$T/big.txt")
  [ "$sz" -ge 40000 ] && ok "big write completed via chunking (${sz}B)" || no "big write too small" "${sz}B — chunking may have failed: $(echo "$OUT" | tail -1)"
else
  no "big write" "no file produced (likely empty-call abort): $(echo "$OUT" | tail -1)"
fi

# 4. Subagent live-fire: spawn a real child via spawn.sh, gate on the envelope.
PF="$T/task.txt"
printf 'Write the single word HATCHED to the file %s. Then end your turn with a VERIFY block: contains=%s:HATCHED\nSUCCESS: %s contains HATCHED\nEXPECT-VERIFY:\n  contains=%s:HATCHED\n' "$T/child.txt" "$T/child.txt" "$T/child.txt" "$T/child.txt" > "$PF"
ENV=$(LAZAR_BIN="$BIN" LAZAR_SPAWN_TIMEOUT=90 EVAL_MODEL="$MODEL" \
  env ANTHROPIC_API_KEY="$ANTHROPIC_API_KEY" ANTHROPIC_BASE_URL="${ANTHROPIC_BASE_URL:-}" \
  bash "$LAZAR_HOME/skills/_meta/subagent/spawn.sh" evalchild "$PF" 2>&1)
if echo "$ENV" | rg -q '"matched":true' && [ -f "$T/child.txt" ] && rg -q HATCHED "$T/child.txt" 2>/dev/null; then
  ok "subagent LIVE-FIRE: child ran, claim matched, envelope ok"
else
  no "subagent live-fire" "envelope=$ENV childfile=$( [ -f "$T/child.txt" ] && cat "$T/child.txt" || echo MISSING)"
fi

# 5. Spawn-discipline veto (deterministic — pipe payloads to the LIVE hook).
HOOK="$LAZAR_HOME/hooks/pre-tool.d/10-spawn-discipline.strict.sh"
if [ -x "$HOOK" ]; then
  V=$(printf '{"command":"LAZAR_DEPTH=1 lazar -p hi"}' | bash "$HOOK" 2>&1)
  echo "$V" | rg -q '"action":"veto"' && ok "veto blocks raw lazar -p (live hook)" || no "veto block" "$V"
  A=$(printf '{"command":"bash skills/_meta/subagent/spawn.sh t /tmp/p"}' | bash "$HOOK" 2>&1)
  echo "$A" | rg -q '"action":"continue"' && ok "veto allows spawn.sh wrapper" || no "veto allow" "$A"
  B=$(printf '{"command":"echo subagent/spawn.sh; lazar -p evil"}' | bash "$HOOK" 2>&1)
  echo "$B" | rg -q '"action":"veto"' && ok "veto blocks decoy spawn.sh + raw lazar -p" || no "veto decoy bypass" "$B"
else
  sk "spawn-veto" "hook not installed"
fi

echo "---- evals: $PASS passed, $FAIL failed, $SKIP skipped ----"
[ "$FAIL" -eq 0 ]
