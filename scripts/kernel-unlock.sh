#!/usr/bin/env bash
# Unlock the lazar kernel source/binary for editing. Pair with kernel-build.sh.
set -euo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

case "$LAZAR_HOME" in
    ""|"/"|"$HOME") echo "error: unsafe LAZAR_HOME: $LAZAR_HOME" >&2; exit 1 ;;
    /*) ;;
    *) echo "error: LAZAR_HOME must be absolute: $LAZAR_HOME" >&2; exit 1 ;;
esac

echo "[kernel-unlock] unlocking $LAZAR_HOME"
chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
chmod u+w      "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
chmod -R u+w   "$LAZAR_HOME/src"

echo "[kernel-unlock] done. Edit $LAZAR_HOME/src/src/main.rs (or seed-skills/) freely."
echo "[kernel-unlock] when ready, run:  bash $LAZAR_HOME/scripts/kernel-build.sh"
