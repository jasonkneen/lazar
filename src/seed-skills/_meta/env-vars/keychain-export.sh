#!/usr/bin/env bash
# Export all present lazar Keychain keys into the current shell.
# Usage:  source skills/_meta/env-vars/keychain-export.sh
# Or:     eval "$(bash skills/_meta/env-vars/keychain-export.sh --print)"
#
# When sourced: sets export VAR=... for each key found.
# When executed: with --print, emits export lines (safe to eval).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/keychain-common.sh"

PRINT=0
[ "${1:-}" = "--print" ] && PRINT=1

_emit() {
  local name="$1" val="$2"
  # Use printf %q for safe shell escaping
  if [ "$PRINT" = "1" ] || [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    printf 'export %s=%q\n' "$name" "$val"
  else
    export "$name=$val"
  fi
}

# If executed (not sourced) without --print, force print mode so `eval "$(...)"` works.
if [ "${BASH_SOURCE[0]}" = "${0}" ] && [ "$PRINT" != "1" ]; then
  PRINT=1
fi

COUNT=0
for name in "${LAZAR_KEY_NAMES[@]}"; do
  if val=$(kc_get "$name" 2>/dev/null); then
    _emit "$name" "$val"
    COUNT=$((COUNT + 1))
  fi
done

if [ "$PRINT" != "1" ]; then
  echo "[env-vars] loaded $COUNT keys from Keychain (service=$LAZAR_KC_SERVICE)" >&2
fi
