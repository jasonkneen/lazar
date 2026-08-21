#!/usr/bin/env bash
# Review and apply a full kernel proposal from workspace/proposals/<name>/.
#
# Before displaying the diff, this script atomically moves the proposal into
# .archive/kernel-proposals/. Model-issued sandboxed commands cannot write
# there, so the tree reviewed is the exact tree applied. Backups also live in
# .archive, outside the agent-writable workspace.
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

fail() {
    echo "kernel-apply: error: $*" >&2
    exit 1
}

case "$LAZAR_HOME" in
    ""|"/"|"$HOME") fail "unsafe LAZAR_HOME: $LAZAR_HOME" ;;
    /*) ;;
    *) fail "LAZAR_HOME must be absolute: $LAZAR_HOME" ;;
esac
HOME_REAL=$(cd "$LAZAR_HOME" 2>/dev/null && pwd -P) ||
    fail "cannot resolve LAZAR_HOME: $LAZAR_HOME"
LAZAR_HOME="$HOME_REAL"

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <proposal-name>" >&2
    echo "       proposals live under $LAZAR_HOME/user/workspace/proposals/" >&2
    find "$LAZAR_HOME/user/workspace/proposals" -mindepth 1 -maxdepth 1 -type d -print 2>/dev/null || true
    exit 2
fi

NAME="$1"
case "$NAME" in
    ""|.*|*/*|*..*|-*|*[!A-Za-z0-9._-]*) fail "unsafe proposal name: $NAME" ;;
esac

PROPOSALS_ROOT="$LAZAR_HOME/user/workspace/proposals"
PROPOSAL="$PROPOSALS_ROOT/$NAME"
[[ ! -L "$LAZAR_HOME/user/workspace" && -d "$LAZAR_HOME/user/workspace" ]] ||
    fail "$LAZAR_HOME/user/workspace must be a real directory"
[[ ! -L "$PROPOSALS_ROOT" && -d "$PROPOSALS_ROOT" ]] ||
    fail "$PROPOSALS_ROOT must be a real directory"
[[ ! -L "$PROPOSAL" && -d "$PROPOSAL" ]] || fail "$PROPOSAL must be a real directory"
[[ ! -L "$LAZAR_HOME/src" && -d "$LAZAR_HOME/src" ]] ||
    fail "live source tree must be a real directory: $LAZAR_HOME/src"
if find "$LAZAR_HOME/src" -type l -print -quit | grep -q .; then
    fail "live source contains a symbolic link; trusted backups must be self-contained"
fi
command -v rsync >/dev/null 2>&1 || fail "rsync is required for checksum-safe apply and recovery"

ROOT_REAL=$(cd "$PROPOSALS_ROOT" 2>/dev/null && pwd -P) ||
    fail "cannot resolve $PROPOSALS_ROOT"
PROP_REAL=$(cd "$PROPOSAL" && pwd -P)
case "$PROP_REAL" in
    "$ROOT_REAL"/*) ;;
    *) fail "proposal path escapes proposals directory: $PROP_REAL" ;;
esac

validate_proposal() {
    local tree="$1" metadata bytes
    [[ ! -L "$tree" && -d "$tree" ]] || fail "$tree must be a real directory"
    if find "$tree" -type l -print -quit | grep -q .; then
        fail "proposal contains a symbolic link; full source proposals must be self-contained"
    fi
    if find "$tree" -type f -links +1 -print -quit | grep -q .; then
        fail "proposal contains a multiply-linked file; the review snapshot must have unique inodes"
    fi
    [[ -d "$tree/src" ]] || fail "$tree/src/ not found"
    [[ -f "$tree/src/Cargo.toml" ]] || fail "proposal is not a full source tree: missing src/Cargo.toml"
    [[ -f "$tree/src/Cargo.lock" ]] || fail "proposal is not reproducible: missing src/Cargo.lock"
    [[ -f "$tree/README.md" ]] || fail "proposal is missing README.md"
    [[ -f "$tree/verify.sh" ]] || fail "proposal is missing verify.sh; verification is mandatory"
    if grep -qF 'Replace this paragraph with the observed defect' "$tree/README.md"; then
        fail "proposal README.md still contains the generated placeholder"
    fi
    if grep -qF 'verify.sh is still the fail-closed template' "$tree/verify.sh"; then
        fail "proposal verify.sh still contains the fail-closed placeholder"
    fi
    for metadata in "$tree/README.md" "$tree/verify.sh"; do
        bytes=$(wc -c <"$metadata" | tr -d '[:space:]')
        [[ "$bytes" -le 32768 ]] || fail "$metadata is too large to review safely ($bytes bytes)"
    done
}

# Diagnose malformed input before the move, then repeat this entire check on
# the frozen tree. Only the post-move snapshot is trusted.
validate_proposal "$PROPOSAL"

ARCHIVE_ROOT="$LAZAR_HOME/.archive"
SNAPSHOT_ROOT="$ARCHIVE_ROOT/kernel-proposals"
BACKUP_ROOT="$ARCHIVE_ROOT/kernel-backups"

ensure_archive_dir() {
    local path="$1"
    if [[ -e "$path" || -L "$path" ]]; then
        [[ ! -L "$path" && -d "$path" ]] || fail "$path must be a real directory"
    else
        mkdir "$path"
    fi
}

ensure_archive_dir "$ARCHIVE_ROOT"
ensure_archive_dir "$SNAPSHOT_ROOT"
ensure_archive_dir "$BACKUP_ROOT"

STAMP=$(date +%Y%m%d-%H%M%S)
SNAPSHOT="$SNAPSHOT_ROOT/$NAME.$STAMP.$$"
BACKUP_ID="src.$STAMP.$$"
BACKUP="$BACKUP_ROOT/$BACKUP_ID"
BACKUP_TMP="$BACKUP_ROOT/.$BACKUP_ID.tmp"
SOURCE_MUTATED=0
SNAPSHOT_MOVED=0

restore_source() {
    [[ -d "$BACKUP/src" ]] || return 1
    chmod -R u+w "$LAZAR_HOME/src" 2>/dev/null || true
    mkdir -p "$LAZAR_HOME/src"
    rsync -a --checksum --delete --exclude '/target/' "$BACKUP/src/" "$LAZAR_HOME/src/"
}

on_exit() {
    rc=$?
    trap - EXIT INT TERM
    if [[ "$rc" -ne 0 ]]; then
        if [[ -e "$BACKUP_TMP" ]]; then
            chmod -R u+w "$BACKUP_TMP" 2>/dev/null || true
            rm -rf -- "$BACKUP_TMP"
        fi
        if [[ "$SOURCE_MUTATED" == "1" && -d "$BACKUP/src" ]]; then
            echo "[kernel-apply] restoring trusted backup after failure" >&2
            if restore_source; then
                if ! bash "$LAZAR_HOME/scripts/kernel-build.sh"; then
                    echo "[kernel-apply] CRITICAL: source was restored but its binary rebuild failed" >&2
                    echo "[kernel-apply] trusted backup: $BACKUP" >&2
                fi
            else
                echo "[kernel-apply] CRITICAL: could not restore trusted backup $BACKUP" >&2
            fi
        fi
        if [[ "$SNAPSHOT_MOVED" == "1" && -d "$SNAPSHOT" && ! -e "$PROPOSAL" ]]; then
            chmod -R u+w "$SNAPSHOT" 2>/dev/null || true
            if mv "$SNAPSHOT" "$PROPOSAL"; then
                echo "[kernel-apply] returned failed/aborted proposal to $PROPOSAL" >&2
            else
                echo "[kernel-apply] proposal remains quarantined at $SNAPSHOT" >&2
            fi
        fi
    fi
    exit "$rc"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[[ ! -e "$SNAPSHOT" ]] || fail "snapshot path already exists: $SNAPSHOT"

echo "[kernel-apply] freezing proposal at $SNAPSHOT"
SNAPSHOT_MOVED=1
mv "$PROPOSAL" "$SNAPSHOT"
# Close the writable-tree race: this is the exact snapshot that will be shown,
# copied, and executed. No destructive operation touches it before this check.
validate_proposal "$SNAPSHOT"
# Build products are generated, not proposal input. Dropping them here keeps
# the immutable review snapshot small and prevents stale artifacts influencing
# review or recovery.
rm -rf -- "$SNAPSHOT/src/target"
chmod -R a-w "$SNAPSHOT"

echo "=== proposal README ==="
cat "$SNAPSHOT/README.md"
echo "=== proposal verification ==="
cat "$SNAPSHOT/verify.sh"
echo "=== source diff vs current kernel ==="
set +e
diff -ru --exclude target "$LAZAR_HOME/src" "$SNAPSHOT/src"
diff_rc=$?
set -e
[[ "$diff_rc" -le 1 ]] || fail "could not diff the frozen proposal"
echo "=== end review ==="

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
    [[ -t 0 ]] || fail "refusing to apply non-interactively; rerun from a terminal"
    read -r -p "Apply this frozen proposal? [y/N] " answer
fi
case "${answer:-N}" in
    y|Y|yes) ;;
    *) echo "aborted"; exit 1 ;;
esac

echo "[kernel-apply] backing up current source to $BACKUP"
[[ ! -e "$BACKUP" && ! -e "$BACKUP_TMP" ]] || fail "backup path already exists: $BACKUP_ID"
mkdir -p "$BACKUP_TMP"
cp -R "$LAZAR_HOME/src" "$BACKUP_TMP/src"
printf '%s\n' \
    "created_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    "proposal=$NAME" \
    "snapshot=$SNAPSHOT" >"$BACKUP_TMP/metadata.txt"
chmod -R a-w "$BACKUP_TMP"
mv "$BACKUP_TMP" "$BACKUP"

echo "[kernel-apply] applying the frozen source with delete semantics"
SOURCE_MUTATED=1
chmod -R u+w "$LAZAR_HOME/src" 2>/dev/null || true
rsync -a --checksum --delete --exclude '/target/' "$SNAPSHOT/src/" "$LAZAR_HOME/src/"

bash "$LAZAR_HOME/scripts/kernel-build.sh"
LAZAR_PROPOSAL_DIR="$SNAPSHOT" \
    bash "$LAZAR_HOME/scripts/kernel-verify.sh" "$NAME"

SOURCE_MUTATED=0
SNAPSHOT_MOVED=0
echo "[kernel-apply] applied proposal: $NAME"
echo "[kernel-apply] trusted backup: $BACKUP_ID ($BACKUP)"
echo "[kernel-apply] frozen proposal preserved at: $SNAPSHOT"
echo "[kernel-apply] rollback: bash $LAZAR_HOME/scripts/kernel-rollback.sh $BACKUP_ID"
