#!/usr/bin/env bash
# Apply a staged kernel proposal from workspace/proposals/<name>/.
# The agent (or you) writes a full proposed src/ tree under workspace/proposals/<name>/src/.
# This script diffs against the current kernel, prompts for confirmation, then applies + rebuilds.
#
# Usage: bash kernel-apply.sh <proposal-name>
set -euo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

fail() { echo "error: $*" >&2; exit 1; }

if [ $# -lt 1 ]; then
    echo "usage: $0 <proposal-name>"
    echo "       proposals live under $LAZAR_HOME/workspace/proposals/"
    ls -1 "$LAZAR_HOME/workspace/proposals/" 2>/dev/null || echo "(no proposals yet)"
    exit 1
fi

NAME="$1"
case "$NAME" in
    ""|.*|*/*|*..*|-*) fail "unsafe proposal name: $NAME" ;;
esac

PROPOSALS_ROOT="$LAZAR_HOME/workspace/proposals"
PROPOSAL="$PROPOSALS_ROOT/$NAME"

if [ ! -d "$PROPOSAL/src" ]; then
    fail "$PROPOSAL/src/ not found"
fi

ROOT_REAL="$(cd "$PROPOSALS_ROOT" 2>/dev/null && pwd -P)" || fail "cannot resolve $PROPOSALS_ROOT"
PROP_REAL="$(cd "$PROPOSAL" && pwd -P)"
case "$PROP_REAL" in
    "$ROOT_REAL"/*) ;;
    *) fail "proposal path escapes proposals directory: $PROP_REAL" ;;
esac

echo "=== diff vs current kernel ==="
diff -ru --exclude target "$LAZAR_HOME/src" "$PROPOSAL/src" || true
echo "==="

if [ ! -t 0 ]; then
    fail "refusing to apply non-interactively; rerun from a terminal"
fi
read -r -p "Apply this proposal? [y/N] " ans
case "${ans:-N}" in
    y|Y|yes) ;;
    *) echo "aborted."; exit 1;;
esac

BACKUP="$LAZAR_HOME/workspace/kernel-backups/src.$(date +%Y%m%d-%H%M%S)"
mkdir -p "$(dirname "$BACKUP")"

echo "[kernel-apply] backing up current source to $BACKUP"
chmod -R u+w "$LAZAR_HOME/src" 2>/dev/null || true
cp -R "$LAZAR_HOME/src" "$BACKUP"

echo "[kernel-apply] applying proposal with delete semantics"
if command -v rsync >/dev/null 2>&1; then
    rsync -a --delete --exclude target/ "$PROPOSAL/src/" "$LAZAR_HOME/src/"
else
    TMP_SRC="$LAZAR_HOME/workspace/.kernel-apply-src.$$"
    rm -rf "$TMP_SRC"
    cp -R "$PROPOSAL/src" "$TMP_SRC"
    rm -rf "$TMP_SRC/target"
    rm -rf "$LAZAR_HOME/src"
    mv "$TMP_SRC" "$LAZAR_HOME/src"
fi

if ! bash "$LAZAR_HOME/scripts/kernel-build.sh"; then
    echo "[kernel-apply] build failed; restoring backup" >&2
    chmod -R u+w "$LAZAR_HOME/src" 2>/dev/null || true
    rm -rf "$LAZAR_HOME/src"
    cp -R "$BACKUP" "$LAZAR_HOME/src"
    exit 1
fi

echo "[kernel-apply] applied proposal: $NAME"
echo "[kernel-apply] backup preserved at: $BACKUP"
echo "[kernel-apply] proposal preserved at: $PROPOSAL"
