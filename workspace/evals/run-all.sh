#!/usr/bin/env bash
# run-all.sh — full verification: kernel units, skill selftests, TUI, live agent evals.
#
# The live suite answers: can a model drive the one-tool Rust kernel + skills
# end-to-end? TUI is not in the critical path of live evals.
#
# Usage:
#   bash workspace/evals/run-all.sh              # full + live (loads lazar-env)
#   bash workspace/evals/run-all.sh --no-live    # offline only
#   EVAL_MODELS="MiniMax-M3 MiniMax-M2.7" bash workspace/evals/run-all.sh
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
cd "$LAZAR_HOME"
LIVE=1; [ "${1:-}" = "--no-live" ] && LIVE=0
FAILED=0
line(){ printf '\n=== %s ===\n' "$1"; }

# Load Keychain + MiniMax defaults for live runs
if [ "$LIVE" = "1" ] && [ -z "${ANTHROPIC_API_KEY:-}" ] && [ -f scripts/lazar-env.sh ]; then
  export LAZAR_ENV_QUIET=1
  # shellcheck disable=SC1091
  source scripts/lazar-env.sh || true
fi

line "kernel unit tests"
( cd src && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/lazar-eval-target}" cargo test 2>&1 | rg 'test result' ) || FAILED=1

line "skill selftests"
for s in subagent log-harvest self-patch env-vars; do
  printf '%-14s ' "$s"
  if [ -x "skills/_meta/$s/selftest.sh" ]; then
    bash "skills/_meta/$s/selftest.sh" >/dev/null 2>&1 && echo PASS || { echo FAIL; FAILED=1; }
  elif [ -x "skills/$s/keychain-list.sh" ]; then
    # env-vars lives at skills/env-vars — smoke list
    bash "skills/env-vars/keychain-list.sh" >/dev/null 2>&1 && echo PASS || { echo FAIL; FAILED=1; }
  else
    echo SKIP
  fi
done

line "TUI go tests (interaction layer only)"
( cd workspace/tui && go test ./... 2>&1 | tail -1 ) || FAILED=1

if [ "$LIVE" = "1" ]; then
  if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    echo; echo "(skipping live evals — no ANTHROPIC_API_KEY; use scripts/lazar-env.sh)"
    FAILED=1
  else
    # Default models: flagship M3 (product default) + cheaper regression tiers
    MODELS="${EVAL_MODELS:-MiniMax-M3 MiniMax-M2.5-highspeed MiniMax-M2.7}"
    for m in $MODELS; do
      line "kernel behavioral evals @ $m"
      EVAL_MODEL="$m" bash workspace/evals/kernel-evals.sh 2>&1 | rg '^\[|evals:' || FAILED=1
      line "agent (one-tool+skills) evals @ $m"
      EVAL_MODEL="$m" bash workspace/evals/agent-evals.sh 2>&1 | rg '^\[|agent-evals:' || FAILED=1
    done
  fi
fi

line "RESULT"
if [ "$FAILED" -eq 0 ]; then
  echo "ALL GREEN — kernel + skills + models hold"
  exit 0
else
  echo "SOME FAILURES — see above"
  exit 1
fi
