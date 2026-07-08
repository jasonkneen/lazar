#!/usr/bin/env bash
# List lazar Keychain API keys (names + present/absent + length). Never values.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/keychain-common.sh"

echo "Keychain service: $LAZAR_KC_SERVICE"
printf '%-24s  %-8s  %s\n' "NAME" "STATUS" "CHARS"
printf '%-24s  %-8s  %s\n' "----" "------" "-----"
for name in "${LAZAR_KEY_NAMES[@]}"; do
  if val=$(kc_get "$name" 2>/dev/null); then
    printf '%-24s  %-8s  %s\n' "$name" "present" "${#val}"
  else
    printf '%-24s  %-8s  %s\n' "$name" "missing" "-"
  fi
done
