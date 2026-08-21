#!/usr/bin/env bash
# Store API keys into macOS Keychain from the current environment or .env.
# Never prints secret values.
#
# Usage:
#   bash keychain-store.sh              # load .env, store all set keys
#   bash keychain-store.sh --from-env   # only already-exported env vars
#   NAME=... VALUE=... bash keychain-store.sh --one MINIMAX_API_KEY
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/keychain-common.sh"

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
MODE="files"
ONE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --from-env) MODE="env"; shift ;;
    --one) ONE="$2"; shift 2 ;;
    -h|--help)
      echo "usage: keychain-store.sh [--from-env] [--one NAME]"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [ "$MODE" = "files" ]; then
  # shellcheck disable=SC1090
  [ -f "$LAZAR_HOME/.env" ] && set -a && source "$LAZAR_HOME/.env" && set +a
fi

# Print the name of another provider key (env or Keychain) that holds the same
# value, if any. A secret can only belong to one provider; storing it under a
# second name means an alias like ANTHROPIC_API_KEY="$MINIMAX_API_KEY" leaked
# into the store and every request to the first provider will 401.
duplicate_of() {
  local name="$1" val="$2" other other_val
  for other in "${LAZAR_KEY_NAMES[@]}"; do
    [ "$other" = "$name" ] && continue
    other_val=$(printenv "$other" 2>/dev/null || true)
    [ -z "$other_val" ] && other_val=$(kc_get "$other" 2>/dev/null || true)
    if [ -n "$other_val" ] && [ "$other_val" = "$val" ]; then
      printf '%s' "$other"
      return 0
    fi
  done
  return 1
}

store_one() {
  local name="$1"
  local val dup
  val=$(printenv "$name" 2>/dev/null || true)
  if [ -z "${val:-}" ]; then
    echo "  skip  $name (not set)"
    return 2
  fi
  if dup=$(duplicate_of "$name" "$val"); then
    echo "  FAIL  $name (value is identical to $dup — refusing to store one provider's key under another provider's name)"
    return 1
  fi
  if kc_set "$name" "$val"; then
    echo "  store $name (${#val} chars)"
    return 0
  else
    echo "  FAIL  $name (security add-generic-password failed)"
    return 1
  fi
}

echo "Storing keys → Keychain service='$LAZAR_KC_SERVICE'"
OK=0; SKIP=0; FAIL=0

if [ -n "$ONE" ]; then
  store_one "$ONE"; rc=$?
  case "$rc" in
    0) OK=$((OK+1)) ;;
    2) SKIP=$((SKIP+1)) ;;
    *) FAIL=$((FAIL+1)) ;;
  esac
else
  for name in "${LAZAR_KEY_NAMES[@]}"; do
    store_one "$name"; rc=$?
    case "$rc" in
      0) OK=$((OK+1)) ;;
      2) SKIP=$((SKIP+1)) ;;
      *) FAIL=$((FAIL+1)) ;;
    esac
  done
fi

echo "---- stored=$OK skipped=$SKIP failed=$FAIL ----"
[ "$FAIL" -eq 0 ]
