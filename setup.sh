#!/usr/bin/env bash
# lazar setup — builds the kernel, locks it, seeds the agent.
# Run from the unpacked tree: bash setup.sh
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "[lazar] target home: $LAZAR_HOME"

# 1. directory layout
mkdir -p "$LAZAR_HOME"/{skills,memory,workspace,logs,bin}

# 2. stage the source
if [[ ! -d "$LAZAR_HOME/src" || "$SCRIPT_DIR/src" -nt "$LAZAR_HOME/src" ]]; then
    echo "[lazar] copying source into $LAZAR_HOME/src"
    # if a previous build sealed the source, unseal it before overwriting
    chmod -R u+w "$LAZAR_HOME/src" 2>/dev/null || true
    rm -rf "$LAZAR_HOME/src"
    cp -R "$SCRIPT_DIR/src" "$LAZAR_HOME/src"
fi

# 3. build (requires Rust toolchain)
if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found. Install Rust from https://rustup.rs/" >&2
    exit 1
fi

echo "[lazar] cargo build --release (first build pulls deps; may take a few minutes)"
( cd "$LAZAR_HOME/src" && cargo build --release )

# 4. install the binary, unlocking any prior immutable copy
echo "[lazar] installing binary"
chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
chmod +w "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
cp "$LAZAR_HOME/src/target/release/lazar" "$LAZAR_HOME/bin/lazar"

# 5. lock the kernel — this is the "cannot be changed once built" property
chmod 555 "$LAZAR_HOME/bin/lazar"
chflags uchg "$LAZAR_HOME/bin/lazar"
chmod -R a-w "$LAZAR_HOME/src"

# 6. factory seed (also exercises the binary as a smoke test)
echo "[lazar] seeding skills via --reset-all --yes"
"$LAZAR_HOME/bin/lazar" --reset-all --yes

# 7. PATH symlink (best-effort)
if [[ -w "/usr/local/bin" ]]; then
    ln -sf "$LAZAR_HOME/bin/lazar" /usr/local/bin/lazar
    echo "[lazar] symlinked /usr/local/bin/lazar -> $LAZAR_HOME/bin/lazar"
else
    echo "[lazar] /usr/local/bin not writable; add to PATH yourself:"
    echo "        export PATH=\"$LAZAR_HOME/bin:\$PATH\""
fi

cat <<EOF

[lazar] built and locked.

  api key:   export ANTHROPIC_API_KEY=sk-...
  use:       lazar -p "your prompt here"
  reset:     lazar --reset-all
  rebuild:   chflags nouchg $LAZAR_HOME/bin/lazar
             chmod +w $LAZAR_HOME/bin/lazar
             chmod -R u+w $LAZAR_HOME/src
             bash $SCRIPT_DIR/setup.sh

EOF
