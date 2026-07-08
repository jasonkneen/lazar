#!/usr/bin/env bash
# stage-proposal.sh — stage a full src/ copy for a KERNEL proposal.
# self-patch never edits the locked src/ directly; it stages a writable copy
# under workspace/proposals/<name>/src/, which a worker (or lazar) then edits
# and dry-builds. The operator applies it later with kernel-apply.sh.
#
# Usage: stage-proposal.sh <name>
# Prints the staged src path on success. Refuses to clobber an existing proposal.
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
NAME="${1:-}"

die(){ echo "self-patch/stage-proposal: $*" >&2; exit 1; }
case "$NAME" in
  ""|.*|*/*|*..*|-*) die "unsafe or missing proposal name: '${NAME:-}'";;
esac
[ -d "$LAZAR_HOME/src" ] || die "no src/ at $LAZAR_HOME/src"

DEST="$LAZAR_HOME/workspace/proposals/$NAME"
[ -e "$DEST/src" ] && die "proposal '$NAME' already staged at $DEST/src — pick a new name or remove it"
mkdir -p "$DEST"

# Copy src/. The live src/ is chmod-locked (read-only), so the copy inherits
# read-only perms — make it writable FIRST, then drop the heavy build target
# (dry-build regenerates it). Order matters: can't rm target/ while read-only.
cp -R "$LAZAR_HOME/src" "$DEST/src"
chmod -R u+w "$DEST/src" 2>/dev/null || true
rm -rf "$DEST/src/target"

echo "$DEST/src"
