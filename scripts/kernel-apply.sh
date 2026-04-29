#!/usr/bin/env bash
# Apply a staged kernel proposal from workspace/proposals/<name>/.
# The agent (or you) writes a full proposed src/ tree under workspace/proposals/<name>/src/.
# This script diffs against the current kernel, prompts for confirmation, then applies + rebuilds.
#
# Usage: bash kernel-apply.sh <proposal-name>
set -euo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

if [ $# -lt 1 ]; then
    echo "usage: $0 <proposal-name>"
    echo "       proposals live under $LAZAR_HOME/workspace/proposals/"
    ls -1 "$LAZAR_HOME/workspace/proposals/" 2>/dev/null || echo "(no proposals yet)"
    exit 1
fi

NAME="$1"
PROPOSAL="$LAZAR_HOME/workspace/proposals/$NAME"

if [ ! -d "$PROPOSAL/src" ]; then
    echo "error: $PROPOSAL/src/ not found" >&2
    exit 1
fi

echo "=== diff vs current kernel ==="
diff -u --recursive "$LAZAR_HOME/src" "$PROPOSAL/src" || true
echo "==="

read -p "Apply this proposal? [y/N] " ans
case "${ans:-N}" in
    y|Y|yes) ;;
    *) echo "aborted."; exit 1;;
esac

bash "$LAZAR_HOME/scripts/kernel-unlock.sh"
cp -R "$PROPOSAL/src/." "$LAZAR_HOME/src/"
bash "$LAZAR_HOME/scripts/kernel-build.sh"

echo "[kernel-apply] applied proposal: $NAME"
echo "[kernel-apply] proposal preserved at: $PROPOSAL"
