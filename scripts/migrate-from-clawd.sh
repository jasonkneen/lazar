#!/usr/bin/env bash
# migrate-from-clawd — move an older install at ~/clawd/lazar to the new
# default home at ~/lazar. Preserves skills/, memory/, workspace/, logs/.
#
# Run from anywhere:
#   bash scripts/migrate-from-clawd.sh [--yes]
#
# Without --yes this script prompts before moving, before updating the global
# symlink, and before patching shell rc files.
set -euo pipefail

OLD="$HOME/clawd/lazar"
NEW="$HOME/lazar"
YES=0

for arg in "$@"; do
    case "$arg" in
        --yes|-y) YES=1 ;;
        *) echo "usage: $0 [--yes]" >&2; exit 2 ;;
    esac
done

confirm() {
    local prompt="$1"
    if [ "$YES" = "1" ]; then
        return 0
    fi
    if [ ! -t 0 ]; then
        echo "[migrate] refusing non-interactive action without --yes: $prompt" >&2
        return 1
    fi
    local ans
    read -r -p "$prompt [y/N] " ans
    case "${ans:-N}" in
        y|Y|yes|YES) return 0 ;;
        *) return 1 ;;
    esac
}

if [ ! -d "$OLD" ]; then
    echo "[migrate] no install found at $OLD — nothing to migrate."
    exit 0
fi

if [ -d "$NEW" ]; then
    echo "[migrate] error: $NEW already exists. Refusing to overwrite." >&2
    echo "[migrate] move or rename it first, then re-run this script." >&2
    exit 1
fi

echo "[migrate] planned move: $OLD  ->  $NEW"
confirm "Move the install directory now?" || { echo "[migrate] aborted."; exit 1; }

# 1. unlock the kernel so we can move it
echo "[migrate] unlocking kernel"
chflags nouchg "$OLD/bin/lazar" 2>/dev/null || true
chmod -R u+w "$OLD" 2>/dev/null || true

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
            if confirm "Update $LINK symlink to $NEW/bin/lazar?"; then
                echo "[migrate] updating $LINK symlink"
                if [ -w "$(dirname "$LINK")" ]; then
                    rm -f "$LINK"
                    ln -s "$NEW/bin/lazar" "$LINK"
                else
                    sudo rm -f "$LINK"
                    sudo ln -s "$NEW/bin/lazar" "$LINK"
                fi
            else
                echo "[migrate] skipped symlink update. Manual command: ln -sf '$NEW/bin/lazar' '$LINK'"
            fi
            ;;
    esac
fi

patch_rc() {
    local rc="$1"
    [ -f "$rc" ] || return 0
    grep -q "clawd/lazar" "$rc" 2>/dev/null || return 0

    local tmp backup
    tmp=$(mktemp "${TMPDIR:-/tmp}/lazar-rc.XXXXXX")
    sed 's|clawd/lazar|lazar|g' "$rc" > "$tmp"
    if cmp -s "$rc" "$tmp"; then
        rm -f "$tmp"
        return 0
    fi

    echo "[migrate] proposed rc update: $rc"
    diff -u "$rc" "$tmp" || true
    if confirm "Apply this rc-file patch?"; then
        backup="${rc}.bak.$(date +%s)"
        cp "$rc" "$backup"
        mv "$tmp" "$rc"
        echo "[migrate] patched $rc (backup at $backup)"
    else
        rm -f "$tmp"
        echo "[migrate] skipped $rc"
    fi
}

# 5. patch shell rc files only after showing the diff / confirming
for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.profile"; do
    patch_rc "$rc"
done

cat <<EOF

[migrate] done. Your install is now at: $NEW

Next steps:
  1. Rebuild the kernel against the new source (the binary will be relocked):
        bash $NEW/scripts/kernel-unlock.sh
        # copy the latest src/ from the repo over $NEW/src/ if you haven't already
        bash $NEW/scripts/kernel-build.sh

  2. Reload your shell so any patched aliases pick up:
        source ~/.zshrc   # or open a new shell

  3. Smoke test:
        lazar -p "what skills do you have?"

EOF
