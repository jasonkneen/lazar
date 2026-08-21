#!/usr/bin/env bash
# lazar setup — installer for the self-evolving agent harness (macOS only).
#
# Safe to re-run: skills/, memory/, workspace/, logs/ and installed sources are
# preserved unless you explicitly opt in below.
#
# Steps:
#   1. Verify platform (Darwin + sandbox-exec + chflags) and toolchain (git, Rust).
#   2. Optionally refresh $LAZAR_HOME/{src,tui,scripts} from this checkout
#      (LAZAR_UPDATE_SOURCE=1), archiving each previous tree first.
#   3. Build the kernel via scripts/kernel-build.sh (external CARGO_TARGET_DIR,
#      atomic bin/lazar swap, chflags uchg, chmod -R a-w src).
#   4. Build the Rust TUI via scripts/tui-build.sh.
#   5. Optionally factory-reset runtime state (LAZAR_RESET_ALL=1 -> lazar --reset-all --yes).
#   6. Install locked helper scripts (chmod 555) and seed skills/hooks/runtime dirs.
#
# Environment:
#   LAZAR_HOME           install/runtime root (default: $PWD; absolute, not / or $HOME)
#   LAZAR_UPDATE_SOURCE  1 = replace installed src/, tui/, and scripts/
#   LAZAR_REFRESH_BUNDLES 1 = archive skills/ + hooks/ and reseed bundled state
#   LAZAR_RESET_ALL      1 = wipe + re-seed runtime state after the build
set -euo pipefail

info() { echo "[setup] $*"; }
fail() { echo "error: $*" >&2; exit 1; }

# --- Platform: macOS only (sandbox-exec profile + chflags uchg guardrails) ---
[[ "$(uname -s)" == "Darwin" ]] ||
    fail "lazar requires macOS (sandbox-exec + chflags uchg). Detected: $(uname -s)"
command -v sandbox-exec >/dev/null 2>&1 ||
    fail "sandbox-exec not found. It ships with macOS; check your PATH."
command -v chflags >/dev/null 2>&1 ||
    fail "chflags not found. It ships with macOS; check your PATH."

# --- Toolchain: git + Rust (pick up a rustup toolchain not yet on PATH) ---
command -v git >/dev/null 2>&1 ||
    fail "git not found. Install it: xcode-select --install (or https://git-scm.com)"
if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi
command -v cargo >/dev/null 2>&1 || fail "cargo not found. Install Rust: https://rustup.rs/"
command -v rustc >/dev/null 2>&1 || fail "rustc not found. Install Rust: https://rustup.rs/"
command -v python3 >/dev/null 2>&1 || fail "python3 not found (required by verification and bundled evals)"
command -v rsync >/dev/null 2>&1 || fail "rsync not found (required by source proposals and recovery)"

# --- Runtime helpers: jq + ripgrep ---
# The installed helper scripts and seeded hooks shell out to both. The seeded
# pre-tool.d/10-spawn-discipline.strict.sh is fail-closed: without jq it
# cannot parse the kernel payload and vetoes EVERY tool call.
command -v jq >/dev/null 2>&1 || fail "jq not found. Install it: brew install jq"
command -v rg >/dev/null 2>&1 || fail "rg (ripgrep) not found. Install it: brew install ripgrep"

# --- Install root ---
LAZAR_HOME="${LAZAR_HOME:-$PWD}"
case "$LAZAR_HOME" in
    ""|"/"|"$HOME") fail "unsafe LAZAR_HOME: $LAZAR_HOME" ;;
    /*) ;;
    *) fail "LAZAR_HOME must be absolute: $LAZAR_HOME" ;;
esac
export LAZAR_HOME

ensure_real_dir() {
    local path="$1"
    if [[ -e "$path" || -L "$path" ]]; then
        [[ ! -L "$path" && -d "$path" ]] || fail "$path must be a real directory"
    else
        mkdir "$path"
    fi
}

ensure_archive_root() {
    ensure_real_dir "$LAZAR_HOME/.archive"
}

# --- Assumes we are running from the clone root ---
[[ -f "src/Cargo.toml" ]] ||
    fail "run this from the lazar clone root (no src/Cargo.toml in $PWD)"

ensure_real_dir "$LAZAR_HOME"
if [[ -e "$LAZAR_HOME/.guardrails" || -L "$LAZAR_HOME/.guardrails" ]]; then
    [[ -f "$LAZAR_HOME/.guardrails" && ! -L "$LAZAR_HOME/.guardrails" ]] || fail "guardrails state must be a regular file"
    PREFLIGHT_MODE=$(sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$LAZAR_HOME/.guardrails")
    [[ "$PREFLIGHT_MODE" == on || "$PREFLIGHT_MODE" == off ]] || fail "invalid persisted guardrails mode"
fi

# --- Optional: refresh the installed source from this checkout ---
if [[ "${LAZAR_UPDATE_SOURCE:-0}" == "1" ]]; then
    if [[ "$(cd "$LAZAR_HOME" 2>/dev/null && pwd -P)" == "$(pwd -P)" ]]; then
        info "LAZAR_HOME is this checkout; installed sources are already current"
    else
        SOURCE_BACKUP_STAMP="$(date +%Y%m%d-%H%M%S).$$"
        ensure_archive_root
        ensure_real_dir "$LAZAR_HOME/.archive/source-updates"
        SOURCE_BACKUP_ROOT="$LAZAR_HOME/.archive/source-updates/$SOURCE_BACKUP_STAMP"
        [[ ! -e "$SOURCE_BACKUP_ROOT" && ! -L "$SOURCE_BACKUP_ROOT" ]] ||
            fail "source archive already exists: $SOURCE_BACKUP_ROOT"
        mkdir "$SOURCE_BACKUP_ROOT"
        for dir in src tui scripts; do
            if [[ -d "$LAZAR_HOME/$dir" ]]; then
                SOURCE_BACKUP="$SOURCE_BACKUP_ROOT/$dir"
                info "archiving existing $dir to $SOURCE_BACKUP"
                mkdir "$SOURCE_BACKUP"
                rsync -a "$LAZAR_HOME/$dir/" "$SOURCE_BACKUP/"
                chmod -R a-w "$SOURCE_BACKUP"
                chmod -R u+w "$LAZAR_HOME/$dir"
            else
                info "no existing $dir at $LAZAR_HOME/$dir; nothing to back up"
            fi
            info "updating $LAZAR_HOME/$dir from this checkout"
            mkdir -p "$LAZAR_HOME/$dir"
            rsync -a --checksum --delete --exclude '/target/' "$dir/" "$LAZAR_HOME/$dir/"
        done
    fi
fi

# --- Installing into a LAZAR_HOME other than this checkout ---
# kernel-build.sh compiles $LAZAR_HOME/src and the ceremony scripts expect
# $LAZAR_HOME/scripts, so seed both on first install instead of failing with
# "source tree missing". Existing trees are left alone (LAZAR_UPDATE_SOURCE=1
# refreshes src/, tui/, and scripts/ explicitly, with trusted archives).
if [[ "$(cd "$LAZAR_HOME" 2>/dev/null && pwd -P)" != "$(pwd -P)" ]]; then
    for dir in src scripts tui; do
        [[ -d "$LAZAR_HOME/$dir" ]] && continue
        info "seeding $LAZAR_HOME/$dir from this checkout"
        mkdir -p "$LAZAR_HOME/$dir"
        rsync -a --exclude '/target/' "$dir/" "$LAZAR_HOME/$dir/"
    done
fi

# Keep the compiled source and installed helpers in the same generation.
for required in scripts/kernel-build.sh scripts/tui-build.sh scripts/lazar-spawn.sh scripts/lazar-failover.sh scripts/run-with-timeout.py scripts/spawn-supervisor.py src/seed-skills/provider-failover/SKILL.md; do
    [[ -f "$LAZAR_HOME/$required" ]] || fail "installed source lacks $required; rerun with LAZAR_UPDATE_SOURCE=1 to archive and update the source generation"
done

# --- Build the kernel (delegates: external target dir, atomic swap, re-lock) ---
info "building kernel via scripts/kernel-build.sh"
bash "$LAZAR_HOME/scripts/kernel-build.sh"

# --- Build the native TUI (separate crate and target directory) ---
info "building TUI via scripts/tui-build.sh"
bash "$LAZAR_HOME/scripts/tui-build.sh"

# --- Optional: factory reset of runtime state (the kernel itself is untouched) ---
if [[ "${LAZAR_RESET_ALL:-0}" == "1" ]]; then
    info "LAZAR_RESET_ALL=1 — resetting skills/hooks/memory/workspace/logs"
    "$LAZAR_HOME/bin/lazar" --reset-all --yes
fi

# --- Install trusted helper surfaces ---
# scripts/ contains the tracked sources; bin/ holds installed copies. bin/ sits
# outside the agent-writable sandbox paths, so model-issued bash cannot replace
# these helpers. Refresh a copy only when it is missing or differs.
mkdir -p "$LAZAR_HOME/bin"
install_locked_helper() {
    local source_path="$1"
    local installed_name="$2"
    local destination="$LAZAR_HOME/bin/$installed_name"
    local temporary="$LAZAR_HOME/bin/.$installed_name.$$.tmp"
    if [[ ! -f "$destination" ]] || ! cmp -s "$source_path" "$destination"; then
        info "installing $destination (chmod 555)"
        cp "$source_path" "$temporary"
        chmod 555 "$temporary"
        # Clear an operator-set immutable flag so the swap can land.
        chflags nouchg "$destination" 2>/dev/null || true
        mv -f "$temporary" "$destination"
    else
        info "bin/$installed_name already up to date"
    fi
}

install_locked_helper "$LAZAR_HOME/scripts/run-with-timeout.py" "run-with-timeout.py"
install_locked_helper "$LAZAR_HOME/scripts/spawn-supervisor.py" "spawn-supervisor.py"
install_locked_helper "$LAZAR_HOME/scripts/lazar-spawn.sh" "lazar-spawn"
install_locked_helper "$LAZAR_HOME/scripts/lazar-failover.sh" "lazar-failover"

# --- Seed runtime state (idempotent; never clobbers existing data) ---
if [[ "${LAZAR_REFRESH_BUNDLES:-0}" == "1" ]]; then
    BUNDLE_ARCHIVE_STAMP="$(date +%Y%m%d-%H%M%S).$$"
    ensure_archive_root
    ensure_real_dir "$LAZAR_HOME/.archive/runtime-bundles"
    BUNDLE_ARCHIVE="$LAZAR_HOME/.archive/runtime-bundles/$BUNDLE_ARCHIVE_STAMP"
    [[ ! -e "$BUNDLE_ARCHIVE" && ! -L "$BUNDLE_ARCHIVE" ]] ||
        fail "bundle archive already exists: $BUNDLE_ARCHIVE"
    mkdir "$BUNDLE_ARCHIVE"
    for bundle in skills hooks; do
        [[ -e "$LAZAR_HOME/$bundle" || -L "$LAZAR_HOME/$bundle" ]] || continue
        [[ ! -L "$LAZAR_HOME/$bundle" && -d "$LAZAR_HOME/$bundle" ]] ||
            fail "$LAZAR_HOME/$bundle must be a real directory before it can be archived"
        info "archiving existing $bundle/ to $BUNDLE_ARCHIVE/$bundle"
        mv "$LAZAR_HOME/$bundle" "$BUNDLE_ARCHIVE/$bundle"
    done
    # Runtime skills may contain symlinks. Do not recursively chmod through
    # them; the archive root is already outside the model write allowlist.
    find "$BUNDLE_ARCHIVE" -type f -exec chmod a-w {} +
fi
mkdir -p "$LAZAR_HOME/user"
if [[ ! -f "$LAZAR_HOME/user/skills/INDEX.md" && ! -f "$LAZAR_HOME/skills/INDEX.md" ]]; then
    info "seeding user/skills from src/seed-skills"
    mkdir -p "$LAZAR_HOME/user/skills"
    cp -R "$LAZAR_HOME/src/seed-skills/." "$LAZAR_HOME/user/skills/"
    chmod -R u+w "$LAZAR_HOME/user/skills"
fi
# Existing installations keep their evolved skill tree, but trusted helpers
# must not point at a missing usage contract. Add newly bundled operational
# skills only when absent; never overwrite an operator- or agent-edited copy.
if [[ ! -d "$LAZAR_HOME/user/skills/provider-failover" && ! -d "$LAZAR_HOME/skills/provider-failover" ]]; then
    info "seeding user/skills/provider-failover"
    mkdir -p "$LAZAR_HOME/user/skills"
    cp -R "$LAZAR_HOME/src/seed-skills/provider-failover" "$LAZAR_HOME/user/skills/provider-failover"
    chmod -R u+w "$LAZAR_HOME/user/skills/provider-failover"
fi
if [[ ! -d "$LAZAR_HOME/user/hooks" && ! -d "$LAZAR_HOME/hooks" ]]; then
    info "seeding user/hooks from src/seed-hooks"
    cp -R "$LAZAR_HOME/src/seed-hooks" "$LAZAR_HOME/user/hooks"
    chmod -R u+w "$LAZAR_HOME/user/hooks"
fi
mkdir -p "$LAZAR_HOME/user/memory" "$LAZAR_HOME/user/workspace" "$LAZAR_HOME/user/logs"
# Absent guardrails means sandbox off. An existing on/off choice is kept.
if [[ ! -e "$LAZAR_HOME/user/.guardrails" && ! -e "$LAZAR_HOME/.guardrails" ]]; then
    printf '%s\n' off >"$LAZAR_HOME/user/.guardrails"
    chmod 600 "$LAZAR_HOME/user/.guardrails"
fi
if [[ ! -f "$LAZAR_HOME/user/providers.toml" && ! -f "$LAZAR_HOME/providers.toml" ]]; then
    info "seeding user/providers.toml"
    cp "$LAZAR_HOME/src/providers.toml" "$LAZAR_HOME/user/providers.toml"
fi

find "$LAZAR_HOME/user/hooks" "$LAZAR_HOME/hooks" -type f -name '*.sh' ! -name '*.disabled' -exec chmod +x {} + 2>/dev/null || true

# --- Done ---
cat <<EOF

=========================================================
  lazar is installed at $LAZAR_HOME
=========================================================

Next steps:

  export ANTHROPIC_API_KEY=sk-...
  export PATH="$LAZAR_HOME/bin:\$PATH"

  lazar -p "what skills do you have?"
  lazartui

Guardrails (kernel/source locks, sandbox mode):

  bash scripts/lazar-guardrails.sh status

Re-running setup is safe — skills/, memory/, workspace/, logs/ are kept.
Source refresh archives are kept under .archive/source-updates/.
  LAZAR_UPDATE_SOURCE=1 bash setup.sh   # refresh src/, tui/, scripts/ from this checkout
  LAZAR_REFRESH_BUNDLES=1 bash setup.sh # archive skills/hooks and reseed bundled state
  LAZAR_RESET_ALL=1     bash setup.sh   # factory-reset runtime state
EOF
