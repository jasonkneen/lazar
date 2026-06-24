#!/usr/bin/env bash
# new-proposal.sh <name> — copy the proposal skeleton to a new proposal dir.
set -euo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
NAME="${1:-}"
[ -z "$NAME" ] && { echo "usage: $0 <name>"; exit 2; }
SKEL="$LAZAR_HOME/workspace/proposals/_skeleton"
DEST="$LAZAR_HOME/workspace/proposals/$NAME"
[ -e "$DEST" ] && { echo "proposal '$NAME' already exists at $DEST"; exit 1; }
[ ! -d "$SKEL" ] && { echo "skeleton missing at $SKEL"; exit 1; }
cp -R "$SKEL" "$DEST"
# sed only on README — verify.sh has the marker we leave for the author to find
sed -i '' "s/<name>/$NAME/g" "$DEST/README.md" "$DEST/TODO.md"
echo "✓ created $DEST"
echo "  next: edit README.md, populate src/, edit verify.sh, then:"
echo "        bash scripts/kernel-apply.sh $NAME"
