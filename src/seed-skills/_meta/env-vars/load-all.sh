#!/usr/bin/env bash
# Load non-secret config from .env + all secrets from Keychain.
# Usage: source skills/env-vars/load-all.sh
# Works when sourced from bash OR zsh.
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

# Resolve this script's directory (bash + zsh)
_ENV_VARS_DIR=""
if [ -n "${BASH_SOURCE[0]:-}" ]; then
  _ENV_VARS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
elif [ -n "${ZSH_VERSION:-}" ]; then
  _ENV_VARS_DIR="$(cd "$(dirname "${(%):-%x}")" && pwd)" 2>/dev/null || true
fi
if [ -z "$_ENV_VARS_DIR" ] || [ ! -f "$_ENV_VARS_DIR/keychain-export.sh" ]; then
  _ENV_VARS_DIR="$LAZAR_HOME/skills/env-vars"
fi

if [ -f "$LAZAR_HOME/.env" ]; then
  set -a
  # shellcheck disable=SC1090
  source "$LAZAR_HOME/.env"
  set +a
fi

if [ -f "$_ENV_VARS_DIR/keychain-export.sh" ]; then
  # Prefer eval print — works under bash and zsh without relying on "source" semantics
  eval "$(bash "$_ENV_VARS_DIR/keychain-export.sh" --print 2>/dev/null)"
else
  echo "[env-vars] WARN: missing keychain-export.sh under $_ENV_VARS_DIR" >&2
fi
unset _ENV_VARS_DIR
