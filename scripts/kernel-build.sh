#!/usr/bin/env bash
# Rebuild the kernel and relock. Run after editing src/.
# If the build fails, the source stays writable so you can fix and retry.
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$LAZAR_HOME/workspace/.cargo-target}"

fail() { echo "error: $*" >&2; exit 1; }

case "$LAZAR_HOME" in
    ""|"/"|"$HOME") fail "unsafe LAZAR_HOME: $LAZAR_HOME" ;;
    /*) ;;
    *) fail "LAZAR_HOME must be absolute: $LAZAR_HOME" ;;
esac

command -v cargo >/dev/null 2>&1 || fail "cargo not found. Install Rust: https://rustup.rs/"
command -v chflags >/dev/null 2>&1 || fail "chflags not found"
[[ -d "$LAZAR_HOME/src" ]] || fail "source tree missing at $LAZAR_HOME/src"

mkdir -p "$CARGO_TARGET_DIR" "$LAZAR_HOME/bin"

echo "[kernel-build] cargo build --release (target: $CARGO_TARGET_DIR)"
( cd "$LAZAR_HOME/src" && CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo build --release )

BUILT="$CARGO_TARGET_DIR/release/lazar"
[[ -x "$BUILT" ]] || fail "built binary missing at $BUILT"

echo "[kernel-build] swapping in new binary atomically"
TMP_BIN="$LAZAR_HOME/bin/.lazar.$$.tmp"
rm -f "$TMP_BIN"
cp "$BUILT" "$TMP_BIN"
chmod 555 "$TMP_BIN"
chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
mv -f "$TMP_BIN" "$LAZAR_HOME/bin/lazar"

# Guardrails: re-lock unless the operator has opted out.
# LAZAR_GUARDRAILS=off keeps bin/ + src/ writable after a rebuild.
if [ "${LAZAR_GUARDRAILS:-on}" = "off" ]; then
    chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
    chmod u+w "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
    chmod -R u+w "$LAZAR_HOME/src"
    echo "[kernel-build] done. Kernel rebuilt — LEFT UNLOCKED (LAZAR_GUARDRAILS=off)."
else
    chflags uchg "$LAZAR_HOME/bin/lazar"
    chmod -R a-w "$LAZAR_HOME/src"
    echo "[kernel-build] done. Kernel rebuilt and locked."
fi
echo "[kernel-build] try:  lazar -p 'hello'"
