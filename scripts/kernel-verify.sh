#!/usr/bin/env bash
# Run mandatory patch-specific verification for a kernel proposal.
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
VERIFY_TIMEOUT="${VERIFY_TIMEOUT:-60}"

fail() {
    echo "kernel-verify: error: $*" >&2
    exit 2
}

[[ $# -eq 1 ]] || fail "usage: $0 <proposal-name>"
NAME="$1"
case "$NAME" in
    ""|.*|*/*|*..*|-*|*[!A-Za-z0-9._-]*) fail "unsafe proposal name: $NAME" ;;
esac
case "$VERIFY_TIMEOUT" in
    ''|*[!0-9]*) fail "VERIFY_TIMEOUT must be a positive integer" ;;
esac
[[ "$VERIFY_TIMEOUT" -gt 0 ]] || fail "VERIFY_TIMEOUT must be a positive integer"

PROPOSAL="${LAZAR_PROPOSAL_DIR:-$LAZAR_HOME/user/workspace/proposals/$NAME}"
VERIFY_SCRIPT="$PROPOSAL/verify.sh"
TIMEOUT_RUNNER="$LAZAR_HOME/scripts/run-with-timeout.py"
[[ -d "$PROPOSAL" ]] || fail "proposal not found: $PROPOSAL"
[[ -f "$VERIFY_SCRIPT" ]] || fail "verify.sh is mandatory: $VERIFY_SCRIPT"
[[ ! -L "$VERIFY_SCRIPT" ]] || fail "verify.sh must not be a symbolic link"
[[ -f "$TIMEOUT_RUNNER" && ! -L "$TIMEOUT_RUNNER" ]] ||
    fail "trusted timeout runner is missing: $TIMEOUT_RUNNER"
command -v python3 >/dev/null 2>&1 || fail "python3 is required for bounded verification"

bytes=$(wc -c <"$VERIFY_SCRIPT" | tr -d '[:space:]')
[[ "$bytes" -le 32768 ]] || fail "verify.sh exceeds the 32768-byte review limit"

echo "=== kernel-verify: running $NAME/verify.sh (timeout ${VERIFY_TIMEOUT}s) ==="
export LAZAR_HOME
export LAZAR_PROPOSAL_NAME="$NAME"
export LAZAR_PROPOSAL_DIR="$PROPOSAL"

set +e
python3 "$TIMEOUT_RUNNER" --timeout "$VERIFY_TIMEOUT" -- bash "$VERIFY_SCRIPT"
rc=$?
set -e

case "$rc" in
    0) echo "=== kernel-verify: PASS ==="; exit 0 ;;
    124) echo "=== kernel-verify: TIMEOUT after ${VERIFY_TIMEOUT}s ===" >&2; exit 1 ;;
    *) echo "=== kernel-verify: FAIL (exit $rc) ===" >&2; exit 1 ;;
esac
