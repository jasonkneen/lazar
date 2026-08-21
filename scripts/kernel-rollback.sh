#!/usr/bin/env bash
# Restore a trusted source backup created by kernel-apply.sh, then rebuild.
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

fail() {
    echo "kernel-rollback: error: $*" >&2
    exit 1
}

case "$LAZAR_HOME" in
    ""|"/"|"$HOME") fail "unsafe LAZAR_HOME: $LAZAR_HOME" ;;
    /*) ;;
    *) fail "LAZAR_HOME must be absolute: $LAZAR_HOME" ;;
esac
LAZAR_HOME=$(cd "$LAZAR_HOME" 2>/dev/null && pwd -P) || fail "cannot resolve LAZAR_HOME"
BACKUP_ROOT="$LAZAR_HOME/.archive/kernel-backups"
command -v rsync >/dev/null 2>&1 || fail "rsync is required for checksum-safe rollback"
[[ ! -L "$LAZAR_HOME/src" && -d "$LAZAR_HOME/src" ]] ||
    fail "live source tree must be a real directory: $LAZAR_HOME/src"
[[ ! -L "$LAZAR_HOME/.archive" && -d "$LAZAR_HOME/.archive" ]] ||
    fail "$LAZAR_HOME/.archive must be a real directory"
[[ ! -L "$BACKUP_ROOT" && -d "$BACKUP_ROOT" ]] ||
    fail "$BACKUP_ROOT must be a real directory"

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <backup-id>" >&2
    find "$BACKUP_ROOT" -mindepth 1 -maxdepth 1 -type d -print 2>/dev/null || true
    exit 2
fi

BACKUP_ID="$1"
case "$BACKUP_ID" in
    ""|.*|*/*|*..*|-*|*[!A-Za-z0-9._-]*) fail "unsafe backup id: $BACKUP_ID" ;;
esac
BACKUP="$BACKUP_ROOT/$BACKUP_ID"
[[ -d "$BACKUP/src" ]] || fail "trusted backup not found: $BACKUP/src"
[[ -f "$BACKUP/src/Cargo.toml" ]] || fail "backup is not a full source tree"
[[ -f "$BACKUP/src/Cargo.lock" ]] || fail "backup is not reproducible: missing Cargo.lock"
if find "$BACKUP/src" -type l -print -quit | grep -q .; then
    fail "backup contains a symbolic link"
fi
if find "$BACKUP/src" -type f -links +1 -print -quit | grep -q .; then
    fail "backup contains a multiply-linked file"
fi

echo "=== rollback diff: live source -> $BACKUP_ID ==="
set +e
diff -ru --exclude target "$LAZAR_HOME/src" "$BACKUP/src"
diff_rc=$?
set -e
[[ "$diff_rc" -le 1 ]] || fail "could not diff the backup"
echo "=== end rollback diff ==="

if [[ "${LAZAR_CEREMONY_TEST_CONFIRM:-0}" == "1" ]]; then
    TEST_PARENT="${TMPDIR:-/tmp}"
    TEST_PARENT="${TEST_PARENT%/}"
    TEST_PARENT=$(cd "$TEST_PARENT" 2>/dev/null && pwd -P) ||
        fail "cannot resolve temporary self-test parent"
    case "$LAZAR_HOME" in
        /tmp/lazar-kernel-ceremony-selftest.*|/private/tmp/lazar-kernel-ceremony-selftest.*|"$TEST_PARENT"/lazar-kernel-ceremony-selftest.*)
            answer="yes"
            ;;
        *) fail "LAZAR_CEREMONY_TEST_CONFIRM is restricted to an isolated self-test root" ;;
    esac
else
    [[ -t 0 ]] || fail "refusing to roll back non-interactively; rerun from a terminal"
    read -r -p "Restore $BACKUP_ID and rebuild? [y/N] " answer
fi
case "${answer:-N}" in
    y|Y|yes) ;;
    *) echo "aborted"; exit 1 ;;
esac

STAMP=$(date +%Y%m%d-%H%M%S)
SAFETY_ID="pre-rollback.$STAMP.$$"
SAFETY="$BACKUP_ROOT/$SAFETY_ID"
SAFETY_TMP="$BACKUP_ROOT/.$SAFETY_ID.tmp"
[[ ! -e "$SAFETY" && ! -e "$SAFETY_TMP" ]] || fail "safety backup path already exists"
mkdir -p "$SAFETY_TMP"
cp -R "$LAZAR_HOME/src" "$SAFETY_TMP/src"
printf '%s\n' \
    "created_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    "before_rollback=$BACKUP_ID" >"$SAFETY_TMP/metadata.txt"
chmod -R a-w "$SAFETY_TMP"
mv "$SAFETY_TMP" "$SAFETY"

restore_tree() {
    local source_tree="$1"
    chmod -R u+w "$LAZAR_HOME/src" 2>/dev/null || true
    rsync -a --checksum --delete --exclude '/target/' "$source_tree/" "$LAZAR_HOME/src/"
}

SOURCE_MUTATED=0
on_exit() {
    rc=$?
    trap - EXIT INT TERM
    if [[ "$rc" -ne 0 && "$SOURCE_MUTATED" == "1" && -d "$SAFETY/src" ]]; then
        echo "[kernel-rollback] restoring pre-rollback source after interruption" >&2
        if restore_tree "$SAFETY/src"; then
            if ! bash "$LAZAR_HOME/scripts/kernel-build.sh"; then
                echo "[kernel-rollback] CRITICAL: source restored but binary rebuild failed" >&2
                echo "[kernel-rollback] safety backup: $SAFETY" >&2
            fi
        else
            echo "[kernel-rollback] CRITICAL: could not restore $SAFETY" >&2
        fi
    fi
    exit "$rc"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

SOURCE_MUTATED=1
restore_tree "$BACKUP/src"
if ! bash "$LAZAR_HOME/scripts/kernel-build.sh"; then
    echo "[kernel-rollback] selected backup did not build; restoring pre-rollback source" >&2
    restore_tree "$SAFETY/src"
    if ! bash "$LAZAR_HOME/scripts/kernel-build.sh"; then
        echo "[kernel-rollback] CRITICAL: pre-rollback binary rebuild also failed" >&2
        echo "[kernel-rollback] safety backup: $SAFETY" >&2
    fi
    SOURCE_MUTATED=0
    exit 1
fi

SOURCE_MUTATED=0
echo "[kernel-rollback] restored and rebuilt: $BACKUP_ID"
echo "[kernel-rollback] pre-rollback source preserved as: $SAFETY_ID"
