#!/usr/bin/env bash
# Stage a complete writable kernel proposal under workspace/proposals/.
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
NAME="${1:-}"
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd -P)
TEMPLATE_DIR="$SCRIPT_DIR/proposal-template"

fail() {
    echo "new-proposal: error: $*" >&2
    exit 1
}

[[ $# -eq 1 ]] || fail "usage: $0 <proposal-name>"
case "$NAME" in
    ""|.*|*/*|*..*|-*|*[!A-Za-z0-9._-]*)
        fail "use a safe slug containing only letters, numbers, dot, underscore, or dash"
        ;;
esac
case "$LAZAR_HOME" in
    ""|"/"|"$HOME") fail "unsafe LAZAR_HOME: $LAZAR_HOME" ;;
    /*) ;;
    *) fail "LAZAR_HOME must be absolute: $LAZAR_HOME" ;;
esac

[[ -d "$LAZAR_HOME/src" ]] || fail "source tree missing: $LAZAR_HOME/src"
[[ -f "$LAZAR_HOME/src/Cargo.toml" ]] || fail "source Cargo.toml missing"
[[ -f "$LAZAR_HOME/src/Cargo.lock" ]] || fail "source Cargo.lock missing"
[[ -f "$TEMPLATE_DIR/README.md" ]] || fail "proposal templates missing: $TEMPLATE_DIR"
[[ -f "$TEMPLATE_DIR/TODO.md" ]] || fail "proposal templates missing: $TEMPLATE_DIR"
[[ -f "$TEMPLATE_DIR/verify.sh" ]] || fail "proposal templates missing: $TEMPLATE_DIR"
command -v rsync >/dev/null 2>&1 || fail "rsync is required for complete source proposals"
[[ ! -L "$LAZAR_HOME/user/workspace" && -d "$LAZAR_HOME/user/workspace" ]] ||
    fail "$LAZAR_HOME/user/workspace must be a real directory"
if find "$LAZAR_HOME/src" -type l -print -quit | grep -q .; then
    fail "live source contains a symbolic link; proposals must be self-contained"
fi

DEST="$LAZAR_HOME/user/workspace/proposals/$NAME"
if [[ -e "$LAZAR_HOME/user/workspace/proposals" || -L "$LAZAR_HOME/user/workspace/proposals" ]]; then
    [[ ! -L "$LAZAR_HOME/user/workspace/proposals" && -d "$LAZAR_HOME/user/workspace/proposals" ]] ||
        fail "$LAZAR_HOME/user/workspace/proposals must be a real directory"
fi
[[ ! -e "$DEST" ]] || fail "proposal already exists: $DEST"
CREATED=0
COMPLETE=0
cleanup() {
    rc=$?
    trap - EXIT
    if [[ "$rc" -ne 0 && "$CREATED" == "1" && "$COMPLETE" == "0" ]]; then
        case "$DEST" in
            "$LAZAR_HOME"/workspace/proposals/*) rm -rf -- "$DEST" ;;
        esac
    fi
    exit "$rc"
}
trap cleanup EXIT

mkdir -p "$DEST/src"
CREATED=1
rsync -a --checksum --exclude '/target/' "$LAZAR_HOME/src/" "$DEST/src/"
chmod -R u+w "$DEST/src" 2>/dev/null || true

for template in README.md TODO.md verify.sh; do
    sed "s/<name>/$NAME/g" "$TEMPLATE_DIR/$template" >"$DEST/$template"
done
chmod +x "$DEST/verify.sh"
COMPLETE=1

echo "[new-proposal] staged complete source at $DEST/src" >&2
echo "[new-proposal] edit README.md and replace the fail-closed verify.sh placeholder" >&2
echo "$DEST/src"
