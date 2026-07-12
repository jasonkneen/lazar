#!/usr/bin/env bash
# lazar setup — builds the kernel, locks it, and seeds missing default skills.
# Run from the unpacked tree: bash setup.sh
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$LAZAR_HOME/workspace/.cargo-target}"

fail() { echo "error: $*" >&2; exit 1; }
quote() { printf '%q' "$1"; }

case "$LAZAR_HOME" in
    ""|"/"|"$HOME") fail "unsafe LAZAR_HOME: $LAZAR_HOME" ;;
    /*) ;;
    *) fail "LAZAR_HOME must be absolute: $LAZAR_HOME" ;;
esac

if [[ "$(uname -s)" != "Darwin" ]]; then
    fail "lazar setup requires macOS (sandbox-exec + chflags)"
fi
command -v cargo >/dev/null 2>&1 || fail "cargo not found. Install Rust from https://rustup.rs/"
command -v chflags >/dev/null 2>&1 || fail "chflags not found"
[[ -x /usr/bin/sandbox-exec ]] || fail "sandbox-exec not found at /usr/bin/sandbox-exec"

if [[ ! -d "$SCRIPT_DIR/src" ]]; then
    fail "source tree not found at $SCRIPT_DIR/src"
fi

echo "[lazar] target home: $LAZAR_HOME"
mkdir -p "$LAZAR_HOME"/{skills,memory,workspace,logs,bin,scripts}

# Stage source safely. Reinstalling should not silently delete local kernel edits.
if [[ "$SCRIPT_DIR" != "$LAZAR_HOME" ]]; then
    if [[ ! -d "$LAZAR_HOME/src" ]]; then
        echo "[lazar] first install: copying source into $LAZAR_HOME/src"
        cp -R "$SCRIPT_DIR/src" "$LAZAR_HOME/src"
    elif [[ "${LAZAR_UPDATE_SOURCE:-0}" == "1" ]]; then
        BACKUP="$LAZAR_HOME/src.backup.$(date +%Y%m%d-%H%M%S)"
        echo "[lazar] backing up existing source to $BACKUP"
        chmod -R u+w "$LAZAR_HOME/src" 2>/dev/null || true
        cp -R "$LAZAR_HOME/src" "$BACKUP"
        echo "[lazar] updating source from $SCRIPT_DIR/src (excluding target/)"
        if command -v rsync >/dev/null 2>&1; then
            rsync -a --delete --exclude target/ "$SCRIPT_DIR/src/" "$LAZAR_HOME/src/"
        else
            TMP_SRC="$LAZAR_HOME/src.update-tmp.$(date +%Y%m%d-%H%M%S).$$"
            cp -R "$SCRIPT_DIR/src" "$TMP_SRC"
            rm -rf "$TMP_SRC/target"
            rm -rf "$LAZAR_HOME/src"
            mv "$TMP_SRC" "$LAZAR_HOME/src"
        fi
    else
        echo "[lazar] existing source preserved at $LAZAR_HOME/src"
        echo "[lazar] set LAZAR_UPDATE_SOURCE=1 to update it from this checkout (backup will be created)"
    fi
fi

mkdir -p "$CARGO_TARGET_DIR"
echo "[lazar] cargo build --release (target: $CARGO_TARGET_DIR)"
( cd "$LAZAR_HOME/src" && CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo build --release )

BUILT="$CARGO_TARGET_DIR/release/lazar"
[[ -x "$BUILT" ]] || fail "built binary missing at $BUILT"

# Install atomically: prepare a locked temp binary, then swap it in.
echo "[lazar] installing binary atomically"
TMP_BIN="$LAZAR_HOME/bin/.lazar.$$.tmp"
rm -f "$TMP_BIN"
cp "$BUILT" "$TMP_BIN"
chmod 555 "$TMP_BIN"
chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
mv -f "$TMP_BIN" "$LAZAR_HOME/bin/lazar"
chflags uchg "$LAZAR_HOME/bin/lazar"

# Install helper scripts so printed maintenance commands work from LAZAR_HOME.
if [[ "$SCRIPT_DIR" != "$LAZAR_HOME" ]]; then
    cp -R "$SCRIPT_DIR/scripts/." "$LAZAR_HOME/scripts/"
fi

# Install locked sub-agent spawn surface (outside agent-writable skills/).
# Source of truth: scripts/lazar-spawn.sh. Budget state lives under logs/subagent.
SPAWN_SRC="$LAZAR_HOME/scripts/lazar-spawn.sh"
if [[ ! -f "$SPAWN_SRC" && -f "$SCRIPT_DIR/scripts/lazar-spawn.sh" ]]; then
    SPAWN_SRC="$SCRIPT_DIR/scripts/lazar-spawn.sh"
    mkdir -p "$LAZAR_HOME/scripts"
    cp "$SPAWN_SRC" "$LAZAR_HOME/scripts/lazar-spawn.sh"
    SPAWN_SRC="$LAZAR_HOME/scripts/lazar-spawn.sh"
fi
if [[ -f "$SPAWN_SRC" ]]; then
    echo "[lazar] installing locked bin/lazar-spawn"
    TMP_SPAWN="$LAZAR_HOME/bin/.lazar-spawn.$$.tmp"
    rm -f "$TMP_SPAWN"
    cp "$SPAWN_SRC" "$TMP_SPAWN"
    chmod 555 "$TMP_SPAWN"
    chflags nouchg "$LAZAR_HOME/bin/lazar-spawn" 2>/dev/null || true
    mv -f "$TMP_SPAWN" "$LAZAR_HOME/bin/lazar-spawn"
    chflags uchg "$LAZAR_HOME/bin/lazar-spawn"
    mkdir -p "$LAZAR_HOME/logs/subagent"
else
    echo "[lazar] WARN: scripts/lazar-spawn.sh missing — subagent spawn not installed"
fi

# Non-destructive default seeding. Full reset is explicit via LAZAR_RESET_ALL=1.
if [[ "${LAZAR_RESET_ALL:-0}" == "1" ]]; then
    echo "[lazar] LAZAR_RESET_ALL=1: factory resetting runtime state"
    "$LAZAR_HOME/bin/lazar" --reset-all --yes
elif [[ ! -f "$LAZAR_HOME/skills/INDEX.md" ]]; then
    echo "[lazar] seeding default skills into empty skills/"
    cp -R "$LAZAR_HOME/src/seed-skills/." "$LAZAR_HOME/skills/"
    chmod -R u+w "$LAZAR_HOME/skills"
else
    echo "[lazar] preserving existing skills/memory/workspace/logs (no reset)"
fi

# The kernel source is immutable after the writable runtime skills have been
# seeded. Locking first makes cp preserve read-only modes in skills/.
chmod -R a-w "$LAZAR_HOME/src"

"$LAZAR_HOME/bin/lazar" --help >/dev/null

LINK="/usr/local/bin/lazar"
if [[ -w "/usr/local/bin" ]]; then
    if [[ -e "$LINK" || -L "$LINK" ]]; then
        TARGET="$(readlink "$LINK" 2>/dev/null || true)"
        if [[ "$TARGET" == "$LAZAR_HOME/bin/lazar" ]]; then
            echo "[lazar] symlink already set: $LINK -> $TARGET"
        else
            echo "[lazar] not overwriting existing $LINK (points to: ${TARGET:-regular file})"
            echo "        add to PATH instead: export PATH=\"$LAZAR_HOME/bin:\$PATH\""
        fi
    else
        ln -s "$LAZAR_HOME/bin/lazar" "$LINK"
        echo "[lazar] symlinked $LINK -> $LAZAR_HOME/bin/lazar"
    fi
else
    echo "[lazar] /usr/local/bin not writable; add to PATH yourself:"
    echo "        export PATH=\"$LAZAR_HOME/bin:\$PATH\""
fi

cat <<EOF

[lazar] built and locked.

  api key:   export ANTHROPIC_API_KEY=***
  use:       lazar -p "your prompt here"
  reset:     lazar --reset-all
  rebuild:   chflags nouchg $(quote "$LAZAR_HOME/bin/lazar")
             chmod u+w $(quote "$LAZAR_HOME/bin/lazar")
             chmod -R u+w $(quote "$LAZAR_HOME/src")
             bash $(quote "$LAZAR_HOME/scripts/kernel-build.sh")

EOF
