#!/usr/bin/env bash
# Run patch-specific verification for a kernel proposal.
#
# Looks for $LAZAR_HOME/workspace/proposals/<name>/verify.sh and executes it
# with bounded timeout. Returns 0 on pass, non-zero on fail. Safe to invoke
# standalone (anytime) or from kernel-apply.sh (auto after build).
#
# Usage:
#   bash kernel-verify.sh <proposal-name>
#
# Env:
#   LAZAR_HOME           default: $HOME/lazar
#   VERIFY_TIMEOUT       default: 60 (seconds)
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
VERIFY_TIMEOUT="${VERIFY_TIMEOUT:-60}"
CARGO_BIN_DIR="$(dirname "$(command -v cargo 2>/dev/null || true)")"
CARGO_HOME_DIR="${CARGO_HOME:-${HOME:-/}/.cargo}"
RUSTUP_HOME_DIR="${RUSTUP_HOME:-${HOME:-/}/.rustup}"

fail() { echo "kernel-verify: error: $*" >&2; exit 2; }

if [ $# -lt 1 ]; then
    echo "usage: $0 <proposal-name>" >&2
    exit 2
fi

NAME="$1"
case "$NAME" in
    ""|.*|*/*|*..*|-*) fail "unsafe proposal name: $NAME" ;;
esac

PROPOSAL="$LAZAR_HOME/workspace/proposals/$NAME"
VERIFY_SCRIPT="$PROPOSAL/verify.sh"

if [ ! -d "$PROPOSAL" ]; then
    fail "proposal not found: $PROPOSAL"
fi

if [ ! -f "$VERIFY_SCRIPT" ]; then
    echo "kernel-verify: no verify.sh in proposal — skipping (consider adding one)" >&2
    exit 0
fi

if [ ! -x "$VERIFY_SCRIPT" ]; then
    chmod +x "$VERIFY_SCRIPT" || fail "verify.sh not executable and cannot chmod"
fi

echo "=== kernel-verify: running $NAME/verify.sh (timeout ${VERIFY_TIMEOUT}s) ==="

# Export context the verify script can rely on
export LAZAR_HOME
export LAZAR_PROPOSAL_NAME="$NAME"
export LAZAR_PROPOSAL_DIR="$PROPOSAL"

# Cross-platform timeout: prefer gtimeout (brew coreutils), then timeout.
# Failing closed is safer than executing an untrusted proposal verifier without
# a deadline.
TIMEOUT_CMD=""
if command -v gtimeout >/dev/null 2>&1; then
    TIMEOUT_CMD="gtimeout $VERIFY_TIMEOUT"
elif command -v timeout >/dev/null 2>&1; then
    TIMEOUT_CMD="timeout $VERIFY_TIMEOUT"
fi

[ -n "$TIMEOUT_CMD" ] || fail "no timeout command available (install coreutils or provide timeout)"
[ -n "$CARGO_BIN_DIR" ] && [ "$CARGO_BIN_DIR" != "." ] || fail "cargo not found"

set +e
# Proposal files are written in the agent's workspace. Do not inherit the
# operator environment or credentials when executing their verifier. The
# regular Lazar sandbox still permits the workspace build target needed by
# Rust tests while protecting the kernel and the rest of the home directory.
$TIMEOUT_CMD /usr/bin/sandbox-exec \
    -D "SKILLS_PATH=$LAZAR_HOME/skills" \
    -D "MEMORY_PATH=$LAZAR_HOME/memory" \
    -D "WORKSPACE_PATH=$LAZAR_HOME/workspace" \
    -D "USER_HOME=${HOME:-/}" \
    -f "$LAZAR_HOME/src/sandbox.sb" \
    /usr/bin/env -i \
    PATH="$CARGO_BIN_DIR:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    HOME="$LAZAR_HOME" \
    CARGO_HOME="$CARGO_HOME_DIR" \
    RUSTUP_HOME="$RUSTUP_HOME_DIR" \
    CARGO_NET_OFFLINE=true \
    LAZAR_HOME="$LAZAR_HOME" \
    LAZAR_SKILLS="$LAZAR_HOME/skills" \
    LAZAR_MEMORY="$LAZAR_HOME/memory" \
    LAZAR_WORKSPACE="$LAZAR_HOME/workspace" \
    LAZAR_LOGS="$LAZAR_HOME/logs" \
    LAZAR_PROPOSAL_NAME="$NAME" \
    LAZAR_PROPOSAL_DIR="$PROPOSAL" \
    /bin/bash "$VERIFY_SCRIPT"
rc=$?
set -e

case "$rc" in
    0)   echo "=== kernel-verify: PASS ==="; exit 0 ;;
    124) echo "=== kernel-verify: TIMEOUT after ${VERIFY_TIMEOUT}s ===" >&2; exit 1 ;;
    *)   echo "=== kernel-verify: FAIL (exit $rc) ===" >&2; exit 1 ;;
esac
