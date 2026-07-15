#!/usr/bin/env bash
# run-all.sh — the full verification loop for lazar: kernel unit tests, all skill
# selftests, TUI go tests, and behavioral evals looped across cheap models.
# This is the "is lazar FULLY working" gate.
#
# Source your launcher env (ANTHROPIC_API_KEY etc.) before running the evals,
# or pass --no-live to skip the model-based behavioral evals.
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
cd "$LAZAR_HOME"
LIVE=1; [ "${1:-}" = "--no-live" ] && LIVE=0
FAILED=0
line(){ printf '\n=== %s ===\n' "$1"; }

line "kernel unit tests"
( cd src && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/lazar-eval-target}" cargo test 2>&1 | rg 'test result' ) || FAILED=1

line "skill selftests"
for s in subagent log-harvest self-patch; do
  printf '%-14s ' "$s"
  bash "skills/_meta/$s/selftest.sh" >/dev/null 2>&1 && echo PASS || { echo FAIL; FAILED=1; }
done

line "TUI go tests"
( cd workspace/tui && go test ./... 2>&1 | tail -1 ) || FAILED=1

line "model event analyzer tests"
bash workspace/evals/model-evals.sh --self-test || FAILED=1

if [ "$LIVE" = "1" ]; then
  if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
    # Loop the behavioral evals across cheap models — behavior must hold on each.
    for m in ${EVAL_MODELS:-MiniMax-M2.5-highspeed MiniMax-M2.7}; do
      line "behavioral evals @ $m"
      EVAL_MODEL="$m" bash workspace/evals/kernel-evals.sh 2>&1 | rg '^\[|evals:' || FAILED=1
    done
  else
    echo; echo "(skipping legacy behavioral evals — ANTHROPIC_API_KEY not set)"
  fi
  if [ -n "${ANTHROPIC_API_KEY:-}${MINIMAX_API_KEY:-}${OPENROUTER_API_KEY:-}" ]; then
    line "event-level model grounding evals"
    bash workspace/evals/model-evals.sh || FAILED=1
  else
    echo; echo "(skipping live grounding evals — no supported provider key is set)"
  fi
fi

line "RESULT"
[ "$FAILED" -eq 0 ] && echo "ALL GREEN" || { echo "SOME FAILURES — see above"; exit 1; }
