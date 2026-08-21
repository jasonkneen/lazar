#!/usr/bin/env bash
# Destructive-path tests for proposal freeze, apply rollback, and manual rollback.
# Everything runs under an isolated temporary LAZAR_HOME with a fake builder.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
TEST_PARENT="${TMPDIR:-/tmp}"
TEST_PARENT="${TEST_PARENT%/}"
TEST_ROOT=$(mktemp -d "$TEST_PARENT/lazar-kernel-ceremony-selftest.XXXXXX")
PASS=0

cleanup() {
    case "$TEST_ROOT" in
        "$TEST_PARENT"/lazar-kernel-ceremony-selftest.*|/tmp/lazar-kernel-ceremony-selftest.*|/private/tmp/lazar-kernel-ceremony-selftest.*)
            chmod -R u+w "$TEST_ROOT" 2>/dev/null || true
            rm -rf -- "$TEST_ROOT"
            ;;
        *) echo "refusing unsafe cleanup target: $TEST_ROOT" >&2 ;;
    esac
}
trap cleanup EXIT

ok() {
    PASS=$((PASS + 1))
    echo "[PASS] $1"
}

require() {
    local label="$1"
    shift
    if "$@"; then
        ok "$label"
    else
        echo "[FAIL] $label" >&2
        exit 1
    fi
}

mkdir -p "$TEST_ROOT/src/src" "$TEST_ROOT/scripts" "$TEST_ROOT/bin" \
    "$TEST_ROOT/workspace/proposals"
printf '%s\n' '[package]' 'name = "fake-lazar"' 'version = "0.1.0"' >"$TEST_ROOT/src/Cargo.toml"
printf '%s\n' '# fake lock' >"$TEST_ROOT/src/Cargo.lock"
printf '%s\n' 'old' >"$TEST_ROOT/src/version"
printf '%s\n' 'old' >"$TEST_ROOT/bin/lazar"

cp "$ROOT/scripts/kernel-apply.sh" "$TEST_ROOT/scripts/kernel-apply.sh"
cp "$ROOT/scripts/kernel-verify.sh" "$TEST_ROOT/scripts/kernel-verify.sh"
cp "$ROOT/scripts/kernel-rollback.sh" "$TEST_ROOT/scripts/kernel-rollback.sh"
cp "$ROOT/scripts/run-with-timeout.py" "$TEST_ROOT/scripts/run-with-timeout.py"
cat >"$TEST_ROOT/scripts/kernel-build.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ -f "$LAZAR_HOME/src/BUILD_FAIL" ]]; then
    echo "fake build failure" >&2
    exit 19
fi
echo "fake build version=$(cat "$LAZAR_HOME/src/version")" >&2
mkdir -p "$LAZAR_HOME/bin"
chmod u+w "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
cp "$LAZAR_HOME/src/version" "$LAZAR_HOME/bin/lazar"
chmod 555 "$LAZAR_HOME/bin/lazar"
EOF
chmod +x "$TEST_ROOT/scripts/"*.sh

new_proposal() {
    LAZAR_HOME="$TEST_ROOT" bash "$ROOT/scripts/new-proposal.sh" "$1"
}

apply_proposal() {
    LAZAR_HOME="$TEST_ROOT" LAZAR_CEREMONY_TEST_CONFIRM=1 \
        VERIFY_TIMEOUT="${VERIFY_TIMEOUT:-60}" \
        bash "$TEST_ROOT/scripts/kernel-apply.sh" "$1"
}

GOOD_SRC=$(new_proposal good 2>"$TEST_ROOT/good-stage.err")
printf '%s\n' 'new' >"$GOOD_SRC/version"
printf '%s\n' '# good' 'Tests frozen apply.' >"$GOOD_SRC/../README.md"
cat >"$GOOD_SRC/../verify.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "verify home=$LAZAR_HOME proposal=$LAZAR_PROPOSAL_DIR version=$(cat "$LAZAR_HOME/src/version")" >&2
case "$LAZAR_PROPOSAL_DIR" in
    "$LAZAR_HOME"/.archive/kernel-proposals/good.*) ;;
    *) echo "proposal was not frozen outside workspace" >&2; exit 1 ;;
esac
[[ ! -e "$LAZAR_HOME/user/workspace/proposals/good" ]]
[[ "$(cat "$LAZAR_HOME/src/version")" == "new" ]]
EOF
chmod +x "$GOOD_SRC/../verify.sh"
if ! apply_proposal good >"$TEST_ROOT/good-apply.out" 2>"$TEST_ROOT/good-apply.err"; then
    cat "$TEST_ROOT/good-apply.out" >&2
    cat "$TEST_ROOT/good-apply.err" >&2
    echo "[FAIL] valid proposal did not apply" >&2
    exit 1
fi

require "frozen proposal applied" grep -qx 'new' "$TEST_ROOT/src/version"
require "installed binary rebuilt" grep -qx 'new' "$TEST_ROOT/bin/lazar"
require "proposal removed from writable workspace" test ! -e "$TEST_ROOT/workspace/proposals/good"
require "frozen proposal preserved in archive" bash -c \
    'find "$1/.archive/kernel-proposals" -mindepth 1 -maxdepth 1 -type d -name "good.*" | grep -q .' _ "$TEST_ROOT"
require "trusted backups stay outside workspace" test ! -e "$TEST_ROOT/workspace/kernel-backups"

BACKUP_PATH=$(find "$TEST_ROOT/.archive/kernel-backups" -mindepth 1 -maxdepth 1 -type d -name 'src.*' | head -n 1)
BACKUP_ID=$(basename "$BACKUP_PATH")
require "pre-apply backup captured old source" grep -qx 'old' "$BACKUP_PATH/src/version"

LAZAR_HOME="$TEST_ROOT" LAZAR_CEREMONY_TEST_CONFIRM=1 \
    bash "$TEST_ROOT/scripts/kernel-rollback.sh" "$BACKUP_ID" \
    >"$TEST_ROOT/manual-rollback.out" 2>"$TEST_ROOT/manual-rollback.err"
require "manual rollback restored source" grep -qx 'old' "$TEST_ROOT/src/version"
require "manual rollback rebuilt binary" grep -qx 'old' "$TEST_ROOT/bin/lazar"
require "manual rollback preserved safety copy" bash -c \
    'find "$1/.archive/kernel-backups" -mindepth 1 -maxdepth 1 -type d -name "pre-rollback.*" | grep -q .' _ "$TEST_ROOT"

BAD_SRC=$(new_proposal bad-verify 2>"$TEST_ROOT/bad-stage.err")
printf '%s\n' 'bad' >"$BAD_SRC/version"
printf '%s\n' '# bad verify' 'Tests automatic verification rollback.' >"$BAD_SRC/../README.md"
cat >"$BAD_SRC/../verify.sh" <<'EOF'
#!/usr/bin/env bash
exit 23
EOF
if apply_proposal bad-verify >"$TEST_ROOT/bad-apply.out" 2>"$TEST_ROOT/bad-apply.err"; then
    echo "[FAIL] verification failure unexpectedly applied" >&2
    exit 1
fi
require "verification failure restored source" grep -qx 'old' "$TEST_ROOT/src/version"
require "verification failure restored binary" grep -qx 'old' "$TEST_ROOT/bin/lazar"
require "failed proposal returned for editing" test -d "$TEST_ROOT/workspace/proposals/bad-verify"

BUILD_SRC=$(new_proposal bad-build 2>"$TEST_ROOT/build-stage.err")
printf '%s\n' 'bad-build' >"$BUILD_SRC/version"
printf '%s\n' 'fail' >"$BUILD_SRC/BUILD_FAIL"
printf '%s\n' '# bad build' 'Tests automatic build rollback.' >"$BUILD_SRC/../README.md"
cat >"$BUILD_SRC/../verify.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
if apply_proposal bad-build >"$TEST_ROOT/build-apply.out" 2>"$TEST_ROOT/build-apply.err"; then
    echo "[FAIL] build failure unexpectedly applied" >&2
    exit 1
fi
require "build failure restored source" grep -qx 'old' "$TEST_ROOT/src/version"
require "build failure kept old binary" grep -qx 'old' "$TEST_ROOT/bin/lazar"
require "build-failed proposal returned for editing" test -d "$TEST_ROOT/workspace/proposals/bad-build"

TIMEOUT_SRC=$(new_proposal verify-timeout 2>"$TEST_ROOT/timeout-stage.err")
printf '%s\n' 'timeout' >"$TIMEOUT_SRC/version"
printf '%s\n' '# verify timeout' 'Tests bounded verification recovery.' >"$TIMEOUT_SRC/../README.md"
cat >"$TIMEOUT_SRC/../verify.sh" <<'EOF'
#!/usr/bin/env bash
trap '' TERM
sleep 20
EOF
if VERIFY_TIMEOUT=1 apply_proposal verify-timeout >"$TEST_ROOT/timeout-apply.out" 2>"$TEST_ROOT/timeout-apply.err"; then
    echo "[FAIL] timed-out verification unexpectedly applied" >&2
    exit 1
fi
require "verification timeout restored source" grep -qx 'old' "$TEST_ROOT/src/version"
require "verification timeout returned proposal" test -d "$TEST_ROOT/workspace/proposals/verify-timeout"
require "verification timeout is reported" grep -q 'TIMEOUT after 1s' "$TEST_ROOT/timeout-apply.err"

MISSING_SRC=$(new_proposal missing-verify 2>"$TEST_ROOT/missing-stage.err")
rm "$MISSING_SRC/../verify.sh"
if apply_proposal missing-verify >"$TEST_ROOT/missing-apply.out" 2>"$TEST_ROOT/missing-apply.err"; then
    echo "[FAIL] proposal without verification unexpectedly applied" >&2
    exit 1
fi
require "missing verification fails before source mutation" grep -qx 'old' "$TEST_ROOT/src/version"
require "missing-verification proposal remains editable" test -d "$TEST_ROOT/workspace/proposals/missing-verify"

echo "kernel ceremony self-test passed: $PASS assertions"
