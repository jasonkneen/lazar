#!/usr/bin/env bash
# migrate-from-clawd — move an older install at ~/clawd/lazar to the new
# default home at ~/lazar. Preserves skills/, memory/, workspace/, logs/.
#
# Run from anywhere:
#   bash scripts/migrate-from-clawd.sh
#
# After this completes, you'll still need to rebuild the kernel against
# the updated source. Run scripts/kernel-build.sh from the repo, or just
# re-run setup.sh.
set -euo pipefail

OLD="$HOME/clawd/lazar"
NEW="$HOME/lazar"

if [ ! -d "$OLD" ]; then
    echo "[migrate] no install found at $OLD — nothing to migrate."
    exit 0
fi

if [ -d "$NEW" ]; then
    echo "[migrate] error: $NEW already exists. Refusing to overwrite." >&2
    echo "[migrate] move or rename it first, then re-run this script." >&2
    exit 1
fi

echo "[migrate] $OLD  ->  $NEW"

# 1. unlock the kernel so we can move it
echo "[migrate] unlocking kernel"
chflags nouchg "$OLD/bin/lazar" 2>/dev/null || true
chmod -R u+w   "$OLD" 2>/dev/null || true

# 2. move the directory
echo "[migrate] moving directory"
mv "$OLD" "$NEW"

# 3. clean up empty parent if it's empty
if [ -d "$HOME/clawd" ] && [ -z "$(ls -A "$HOME/clawd" 2>/dev/null)" ]; then
    rmdir "$HOME/clawd"
    echo "[migrate] removed empty $HOME/clawd"
fi

# 4. update /usr/local/bin/lazar symlink if it pointed at the old path
LINK="/usr/local/bin/lazar"
if [ -L "$LINK" ]; then
    TARGET=$(readlink "$LINK" 2>/dev/null || true)
    case "$TARGET" in
        *clawd/lazar*)
            echo "[migrate] updating $LINK symlink (may prompt for sudo)"
            sudo rm -f "$LINK"
            sudo ln -s "$NEW/bin/lazar" "$LINK"
            ;;
    esac
fi

# 5. patch shell rc files in-place if they reference the old path
for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.profile"; do
    if [ -f "$rc" ] && grep -q "clawd/lazar" "$rc" 2>/dev/null; then
        cp "$rc" "${rc}.bak.$(date +%s)"
        sed -i '' 's|clawd/lazar|lazar|g' "$rc" 2>/dev/null \
            || sed -i 's|clawd/lazar|lazar|g' "$rc"
        echo "[migrate] patched $rc (backup at ${rc}.bak.<ts>)"
    fi
done

cat <<EOF

[migrate] done. Your install is now at: $NEW

Next steps:
  1. Rebuild the kernel against the new source (the binary will be relocked):
        bash $NEW/scripts/kernel-unlock.sh
        # copy the latest src/ from the repo over $NEW/src/ if you haven't already
        bash $NEW/scripts/kernel-build.sh

  2. Reload your shell so the patched aliases pick up:
        source ~/.zshrc   # or open a new shell

  3. Smoke test:
        lazar -p "what skills do you have?"

EOF
