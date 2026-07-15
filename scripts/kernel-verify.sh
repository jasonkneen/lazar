#!/usr/bin/env bash
# Run patch-specific verification for a kernel proposal.
#
# Looks for $LAZAR_HOME/workspace/proposals/<name>/verify.sh and executes it
# with bounded timeout. Returns 0 on pass, non-zero on fail. Safe to invoke
# standalone (anytime) or from kernel-apply.sh (auto after build).
#
# Usage:
#   bash kernel-verify.sh <proposal-name>
#
# Env:
#   LAZAR_HOME           default: $HOME/lazar
#   VERIFY_TIMEOUT       default: 60 (seconds)
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
VERIFY_TIMEOUT="${VERIFY_TIMEOUT:-60}"

fail() { echo "kernel-verify: error: $*" >&2; exit 2; }

if [ $# -lt 1 ]; then
    echo "usage: $0 <proposal-name>" >&2
    exit 2
fi

NAME="$1"
case "$NAME" in
    ""|.*|*/*|*..*|-*) fail "unsafe proposal name: $NAME" ;;
esac

PROPOSAL="$LAZAR_HOME/workspace/proposals/$NAME"
VERIFY_SCRIPT="$PROPOSAL/verify.sh"

if [ ! -d "$PROPOSAL" ]; then
    fail "proposal not found: $PROPOSAL"
fi

if [ ! -f "$VERIFY_SCRIPT" ]; then
    echo "kernel-verify: no verify.sh in proposal — skipping (consider adding one)" >&2
    exit 0
fi

if [ ! -x "$VERIFY_SCRIPT" ]; then
    chmod +x "$VERIFY_SCRIPT" || fail "verify.sh not executable and cannot chmod"
fi

echo "=== kernel-verify: running $NAME/verify.sh (timeout ${VERIFY_TIMEOUT}s) ==="

# Export context the verify script can rely on
export LAZAR_HOME
export LAZAR_PROPOSAL_NAME="$NAME"
export LAZAR_PROPOSAL_DIR="$PROPOSAL"

# Cross-platform timeout: prefer timeout, then gtimeout (brew coreutils), else run plain
TIMEOUT_CMD="$(command -v timeout || command -v gtimeout || true)"

set +e
if [ -n "$TIMEOUT_CMD" ]; then
    "$TIMEOUT_CMD" "$VERIFY_TIMEOUT" bash "$VERIFY_SCRIPT"
else
    bash "$VERIFY_SCRIPT"
fi
rc=$?
set -e

case "$rc" in
    0)   echo "=== kernel-verify: PASS ==="; exit 0 ;;
    124) echo "=== kernel-verify: TIMEOUT after ${VERIFY_TIMEOUT}s ===" >&2; exit 1 ;;
    *)   echo "=== kernel-verify: FAIL (exit $rc) ===" >&2; exit 1 ;;
esac
