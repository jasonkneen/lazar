#!/usr/bin/env bash
# Load non-secret config from .env + all secrets from Keychain.
# Usage: source skills/env-vars/load-all.sh
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ -f "$LAZAR_HOME/.env" ]; then
  set -a
  # shellcheck disable=SC1090
  source "$LAZAR_HOME/.env"
  set +a
fi

# shellcheck disable=SC1091
source "$HERE/keychain-export.sh"
