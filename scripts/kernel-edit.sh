#!/usr/bin/env bash
# One-shot: unlock, open main.rs in $EDITOR, rebuild, relock.
# Use this when you want to make a quick kernel edit.
# If the build fails after editing, the kernel stays unlocked so you can fix and rerun build.
set -euo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
EDITOR="${EDITOR:-vi}"

bash "$LAZAR_HOME/scripts/kernel-unlock.sh"

echo "[kernel-edit] opening main.rs in $EDITOR (set \$EDITOR to choose)"
"$EDITOR" "$LAZAR_HOME/src/src/main.rs"

echo "[kernel-edit] proceeding to build"
bash "$LAZAR_HOME/scripts/kernel-build.sh"
