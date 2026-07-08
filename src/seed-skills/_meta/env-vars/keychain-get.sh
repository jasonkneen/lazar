#!/usr/bin/env bash
# Get one key from Keychain and print to stdout (for shell capture only).
# Usage:  KEY=$(bash keychain-get.sh MINIMAX_API_KEY)
#         bash keychain-get.sh MINIMAX_API_KEY --meta   # length only, no value
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/keychain-common.sh"

NAME="${1:-}"
MODE="${2:-}"
[ -n "$NAME" ] || { echo "usage: keychain-get.sh <ENV_VAR_NAME> [--meta]" >&2; exit 2; }

if ! val=$(kc_get "$NAME"); then
  echo "missing: $NAME (service=$LAZAR_KC_SERVICE)" >&2
  exit 1
fi

if [ "$MODE" = "--meta" ]; then
  echo "$NAME present chars=${#val}"
  exit 0
fi

# Value only — caller is responsible for not logging it.
printf '%s' "$val"
