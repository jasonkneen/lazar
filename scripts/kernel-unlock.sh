#!/usr/bin/env bash
# Unlock the lazar kernel for editing. Pair with kernel-build.sh.
set -euo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

echo "[kernel-unlock] unlocking $LAZAR_HOME"
chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
chmod +w       "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
chmod -R u+w   "$LAZAR_HOME/src"

echo "[kernel-unlock] done. Edit ~/lazar/src/src/main.rs (or seed-skills/) freely."
echo "[kernel-unlock] when ready, run:  bash $LAZAR_HOME/scripts/kernel-build.sh"
