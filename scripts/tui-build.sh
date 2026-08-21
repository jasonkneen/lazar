#!/usr/bin/env bash
# Build the Rust TUI and install it atomically as bin/lazartui.
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$LAZAR_HOME/user/workspace/.cargo-target-tui}"

fail() { echo "error: $*" >&2; exit 1; }

case "$LAZAR_HOME" in
    ""|"/"|"$HOME") fail "unsafe LAZAR_HOME: $LAZAR_HOME" ;;
    /*) ;;
    *) fail "LAZAR_HOME must be absolute: $LAZAR_HOME" ;;
esac

command -v cargo >/dev/null 2>&1 || fail "cargo not found. Install Rust: https://rustup.rs/"
[[ -f "$LAZAR_HOME/tui/Cargo.toml" ]] || fail "TUI source missing at $LAZAR_HOME/tui"

mkdir -p "$CARGO_TARGET_DIR" "$LAZAR_HOME/bin"
echo "[tui-build] cargo build --release --locked (target: $CARGO_TARGET_DIR)"
CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo build \
    --release --locked --manifest-path "$LAZAR_HOME/tui/Cargo.toml"

BUILT="$CARGO_TARGET_DIR/release/lazartui"
[[ -x "$BUILT" ]] || fail "built binary missing at $BUILT"

DEST="$LAZAR_HOME/bin/lazartui"
TMP_BIN="$LAZAR_HOME/bin/.lazartui.$$.tmp"
echo "[tui-build] swapping in $DEST atomically"
rm -f "$TMP_BIN"
cp "$BUILT" "$TMP_BIN"
chmod 555 "$TMP_BIN"
chflags nouchg "$DEST" 2>/dev/null || true
mv -f "$TMP_BIN" "$DEST"

echo "[tui-build] done. Try: $DEST"
