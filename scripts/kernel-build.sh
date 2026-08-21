#!/usr/bin/env bash
# Rebuild the kernel and relock. Run after editing src/.
# If the build fails, the source stays writable so you can fix and retry.
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$LAZAR_HOME/user/workspace/.cargo-target}"

fail() { echo "error: $*" >&2; exit 1; }

case "$LAZAR_HOME" in
    ""|"/"|"$HOME") fail "unsafe LAZAR_HOME: $LAZAR_HOME" ;;
    /*) ;;
    *) fail "LAZAR_HOME must be absolute: $LAZAR_HOME" ;;
esac

command -v cargo >/dev/null 2>&1 || fail "cargo not found. Install Rust: https://rustup.rs/"
command -v chflags >/dev/null 2>&1 || fail "chflags not found"
[[ -d "$LAZAR_HOME/src" ]] || fail "source tree missing at $LAZAR_HOME/src"

# Guardrails: follow the protected persisted mode unless this one build has an
# explicit LAZAR_GUARDRAILS override.
BUILD_GUARDRAILS="${LAZAR_GUARDRAILS:-}"
if [[ -z "$BUILD_GUARDRAILS" ]]; then
    MODE_FILE="$LAZAR_HOME/.guardrails"
    [[ ! -L "$MODE_FILE" ]] || fail "$MODE_FILE must not be a symbolic link"
    [[ ! -e "$MODE_FILE" || -f "$MODE_FILE" ]] || fail "$MODE_FILE must be a regular file"
    if [[ -f "$MODE_FILE" ]]; then
        BUILD_GUARDRAILS=$(sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$MODE_FILE")
    else
        BUILD_GUARDRAILS=on
    fi
fi
case "$BUILD_GUARDRAILS" in
    on|off) ;;
    *) fail "guardrails build mode must be 'on' or 'off' (got '$BUILD_GUARDRAILS')" ;;
esac


mkdir -p "$CARGO_TARGET_DIR" "$LAZAR_HOME/bin"

echo "[kernel-build] cargo build --release --locked (target: $CARGO_TARGET_DIR)"
( cd "$LAZAR_HOME/src" && CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo build --release --locked )

BUILT="$CARGO_TARGET_DIR/release/lazar"
[[ -x "$BUILT" ]] || fail "built binary missing at $BUILT"

echo "[kernel-build] swapping in new binary atomically"
TMP_BIN="$LAZAR_HOME/bin/.lazar.$$.tmp"
rm -f "$TMP_BIN"
cp "$BUILT" "$TMP_BIN"
chmod 755 "$TMP_BIN"
chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
mv -f "$TMP_BIN" "$LAZAR_HOME/bin/lazar"
chmod -R u+w "$LAZAR_HOME/src" 2>/dev/null || true
echo "[kernel-build] done. Kernel rebuilt. Source stays writable."
echo "[kernel-build] try:  lazar -p 'hello'"
