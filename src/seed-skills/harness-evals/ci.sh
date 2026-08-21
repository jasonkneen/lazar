#!/usr/bin/env bash
# ci.sh — CI gate for harness quality.
#
#   bash skills/harness-evals/ci.sh            # offline only (free, no API)
#   bash skills/harness-evals/ci.sh --live     # + live required suites
#   bash skills/harness-evals/ci.sh --live --provider kimi --repeat 2
#
# Exit nonzero if a required behavior fails or a required run is incomplete.
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
SKILL="${HARNESS_EVALS_SKILL_DIR:-$LAZAR_HOME/user/skills/harness-evals}"
HE="$SKILL/bin/harness-evals"
LIVE=0; PROVIDER=""; REPEAT=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --live) LIVE=1; shift ;;
    --provider|--backend) PROVIDER="$2"; shift 2 ;;
    --repeat) REPEAT="$2"; shift 2 ;;
    *) echo "ci: unknown arg $1" >&2; exit 2 ;;
  esac
done

FAILED=0
echo "=== 1/2 offline scorer selftest ==="
bash "$SKILL/selftest.sh" || FAILED=1

if [[ "$LIVE" -eq 1 ]]; then
  echo
  echo "=== 2/2 live harness evals (required suites) ==="
  args=(run --suite core,outcomes --repeat "$REPEAT" --label ci)
  [[ -n "$PROVIDER" ]] && args+=(--provider "$PROVIDER")
  bash "$HE" "${args[@]}" || FAILED=1
else
  echo
  echo "=== 2/2 live evals SKIPPED (pass --live to enable) ==="
fi

echo
if [[ "$FAILED" -eq 0 ]]; then
  echo "HARNESS CI: GREEN"
else
  echo "HARNESS CI: RED - failure or incomplete evidence" >&2
fi
exit "$FAILED"
