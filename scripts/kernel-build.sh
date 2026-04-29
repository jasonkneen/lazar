#!/usr/bin/env bash
# Rebuild the kernel and relock. Run after editing src/.
# If the build fails, the kernel stays unlocked so you can fix and retry.
set -euo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found. Install Rust: https://rustup.rs/" >&2
    exit 1
fi

echo "[kernel-build] cargo build --release"
( cd "$LAZAR_HOME/src" && cargo build --release )

echo "[kernel-build] swapping in new binary"
chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
chmod +w       "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
cp "$LAZAR_HOME/src/target/release/lazar" "$LAZAR_HOME/bin/lazar"

echo "[kernel-build] relocking"
chmod 555      "$LAZAR_HOME/bin/lazar"
chflags uchg   "$LAZAR_HOME/bin/lazar"
chmod -R a-w   "$LAZAR_HOME/src"

echo "[kernel-build] done. Kernel rebuilt and locked."
echo "[kernel-build] try:  lazar -p 'hello'"
