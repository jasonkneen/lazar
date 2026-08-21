#!/usr/bin/env bash
# Black-box fresh-install smoke test for setup.sh. Builds into an isolated
# temporary LAZAR_HOME, validates the installed surfaces, then removes it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
TEST_ROOT="$(mktemp -d /tmp/lazar-setup-selftest.XXXXXX)"
INSTALL_ROOT="$TEST_ROOT/install"
BUILD_ROOT="${LAZAR_SETUP_TEST_TARGET:-$TEST_ROOT/target}"

cleanup() {
    case "$TEST_ROOT" in
        /tmp/lazar-setup-selftest.*|/private/tmp/lazar-setup-selftest.*) ;;
        *) echo "refusing unsafe cleanup target: $TEST_ROOT" >&2; return ;;
    esac
    chflags -R nouchg "$TEST_ROOT" 2>/dev/null || true
    chmod -R u+w "$TEST_ROOT" 2>/dev/null || true
    rm -rf -- "$TEST_ROOT"
}
trap cleanup EXIT

mkdir -p "$INSTALL_ROOT"
mkdir -p "$INSTALL_ROOT/skills/obsolete"
printf '%s\n' '# stale index' >"$INSTALL_ROOT/skills/INDEX.md"
printf '%s\n' '# obsolete' >"$INSTALL_ROOT/skills/obsolete/SKILL.md"
mkdir -p "$INSTALL_ROOT/hooks/pre-tool.d"
printf '%s\n' '#!/usr/bin/env bash' >"$INSTALL_ROOT/hooks/pre-tool.d/obsolete.sh"
(
    cd "$ROOT"
    LAZAR_HOME="$INSTALL_ROOT" \
        CARGO_TARGET_DIR="$BUILD_ROOT" \
        LAZAR_REFRESH_BUNDLES=1 \
        bash setup.sh
)

for path in \
    bin/lazar \
    bin/lazartui \
    bin/lazar-spawn \
    bin/lazar-failover \
    skills/INDEX.md \
    hooks \
    .guardrails \
    providers.toml; do
    [[ -e "$INSTALL_ROOT/$path" ]] || {
        echo "missing installed surface: $path" >&2
        exit 1
    }
done
[[ -w "$INSTALL_ROOT/skills/INDEX.md" ]] || { echo "runtime skills are read-only" >&2; exit 1; }

[[ "$(tr -d '[:space:]' <"$INSTALL_ROOT/.guardrails")" == "on" ]] || {
    echo "fresh install did not default guardrails to on" >&2
    exit 1
}

[[ ! -e "$INSTALL_ROOT/skills/obsolete" ]] || {
    echo "skill refresh left the obsolete runtime skill active" >&2
    exit 1
}
find "$INSTALL_ROOT/.archive/runtime-bundles" -path '*/skills/obsolete/SKILL.md' -print -quit |
    grep -q . || {
        echo "skill refresh did not archive the obsolete runtime skill" >&2
        exit 1
    }
find "$INSTALL_ROOT/.archive/runtime-bundles" -path '*/hooks/pre-tool.d/obsolete.sh' -print -quit |
    grep -q . || {
        echo "bundle refresh did not archive the obsolete runtime hook" >&2
        exit 1
    }
[[ ! -e "$INSTALL_ROOT/hooks/pre-tool.d/obsolete.sh" ]] || {
    echo "bundle refresh left the obsolete runtime hook active" >&2
    exit 1
}

LAZAR_HOME="$INSTALL_ROOT" bash "$INSTALL_ROOT/scripts/lazar-guardrails.sh" off >/dev/null
[[ "$(tr -d '[:space:]' <"$INSTALL_ROOT/.guardrails")" == "off" ]]
find "$INSTALL_ROOT/hooks/pre-tool.d" -maxdepth 1 -name '*.guardrails-disabled' -print -quit |
    grep -q .
[[ -e "$INSTALL_ROOT/hooks/pre-tool.d/00-example-deny-rm-rf.sh.disabled" ]]
LAZAR_HOME="$INSTALL_ROOT" bash "$INSTALL_ROOT/scripts/lazar-guardrails.sh" on >/dev/null
[[ "$(tr -d '[:space:]' <"$INSTALL_ROOT/.guardrails")" == "on" ]]
! find "$INSTALL_ROOT/hooks/pre-tool.d" -maxdepth 1 -name '*.guardrails-disabled' -print -quit |
    grep -q .
[[ -e "$INSTALL_ROOT/hooks/pre-tool.d/00-example-deny-rm-rf.sh.disabled" ]]
printf '%s\n' invalid >"$INSTALL_ROOT/.guardrails"
LAZAR_HOME="$INSTALL_ROOT" bash "$INSTALL_ROOT/scripts/lazar-guardrails.sh" on >/dev/null
[[ "$(tr -d '[:space:]' <"$INSTALL_ROOT/.guardrails")" == "on" ]]

"$INSTALL_ROOT/bin/lazar" --help >/dev/null
"$INSTALL_ROOT/bin/lazar" providers --json >/dev/null
"$INSTALL_ROOT/bin/lazartui" --help >/dev/null
"$INSTALL_ROOT/bin/lazar-failover" --help >/dev/null
LAZAR_HOME="$INSTALL_ROOT" \
    bash "$INSTALL_ROOT/skills/provider-failover/selftest.sh" >/dev/null
LAZAR_HOME="$INSTALL_ROOT" \
    bash "$INSTALL_ROOT/skills/_meta/self-patch/selftest.sh" >/dev/null
bash "$INSTALL_ROOT/scripts/kernel-ceremony-selftest.sh" >/dev/null

echo "setup self-test passed"
