#!/usr/bin/env bash
# session-start: tell lazar which enforcement mode it is in.
# Without this the agent guesses from stale prompt text and refuses writes
# that are actually permitted (documented failure, 2026-08-08).
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
STATE=$(cat "$LAZAR_HOME/workspace/.guardrails" 2>/dev/null || echo on)

if [ "$STATE" = "off" ]; then
  cat <<'EOF'
GUARDRAILS: OFF — full operator filesystem access.
- logs/, bin/, src/ are ALL writable. Trusted-zone rules do NOT apply.
- Do NOT refuse a write or suggest a "safer" path on permission grounds.
- Discipline still applies: kernel edits via workspace/proposals/ + scripts/kernel-apply.sh.
- Toggle: bash $LAZAR_HOME/scripts/lazar-guardrails.sh on|off|status  (skill: guardrails)
EOF
else
  cat <<'EOF'
GUARDRAILS: ON — sandboxed.
- Writable: skills/, memory/, workspace/, /tmp. logs/ bin/ src/ are NOT writable.
- If the operator wants a blocked write, do not give up: run
  `bash $LAZAR_HOME/scripts/lazar-guardrails.sh status`, offer to toggle off, then do it.
- Toggle: bash $LAZAR_HOME/scripts/lazar-guardrails.sh on|off|status  (skill: guardrails)
EOF
fi
