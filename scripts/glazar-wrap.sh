#!/usr/bin/env bash
# glazar-wrap.sh — PATH entry for glazar (lazar -p through glow).
# Install: ln -sf "$HOME/lazar/scripts/glazar-wrap.sh" "$HOME/bin/glazar"
set -euo pipefail
if ! command -v glow >/dev/null 2>&1; then
  echo "glazar: glow not found — brew install glow" >&2
  exit 1
fi
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
export LAZAR_HOME LAZAR_ENV_QUIET=1
# shellcheck disable=SC1090
source "$LAZAR_HOME/scripts/lazar-env.sh"
exec "$LAZAR_HOME/bin/lazar" -p "$*" --output-format text 2>/dev/null | glow -
