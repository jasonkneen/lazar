#!/usr/bin/env bash
# lazar-wrap.sh — PATH-safe entrypoint for `lazar` from anywhere.
# Loads MiniMax-M3 defaults + Keychain keys, then execs the real kernel binary.
#
# Install (once):
#   ln -sf "$HOME/lazar/scripts/lazar-wrap.sh" "$HOME/bin/lazar"
# Keep the real binary at $LAZAR_HOME/bin/lazar (never overwrite with this script).

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
export LAZAR_ENV_QUIET="${LAZAR_ENV_QUIET:-1}"
# shellcheck disable=SC1090
source "$LAZAR_HOME/scripts/lazar-env.sh" || {
  echo "lazar-wrap: failed to load lazar-env.sh" >&2
  exit 1
}
if [ ! -x "$LAZAR_HOME/bin/lazar" ]; then
  echo "lazar-wrap: missing kernel binary at $LAZAR_HOME/bin/lazar" >&2
  exit 1
fi
exec "$LAZAR_HOME/bin/lazar" "$@"
