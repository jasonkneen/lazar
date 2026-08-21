#!/usr/bin/env bash
# Shared helpers for lazar Keychain-backed API keys.
# Convention: service="lazar" account="<ENV_VAR_NAME>" (e.g. MINIMAX_API_KEY)
# NEVER print secret values. Status only.
set -uo pipefail

LAZAR_KC_SERVICE="${LAZAR_KC_SERVICE:-lazar}"

# Canonical set of keys lazar may store/verify.
LAZAR_KEY_NAMES=(
  ANTHROPIC_API_KEY
  MINIMAX_API_KEY
  KIMI_API_KEY
  KIMI_CODING_API_KEY
  OPENROUTER_API_KEY
)

kc_get() {
  # usage: kc_get ACCOUNT_NAME  → prints secret to stdout, exit 1 if missing
  local acct="$1"
  security find-generic-password -a "$acct" -s "$LAZAR_KC_SERVICE" -w 2>/dev/null
}

kc_set() {
  # usage: kc_set ACCOUNT_NAME VALUE
  local acct="$1" val="$2"
  # -U updates if exists. -T "" restricts access (default allows this process).
  security add-generic-password -a "$acct" -s "$LAZAR_KC_SERVICE" -w "$val" -U >/dev/null 2>&1
}

kc_has() {
  local acct="$1"
  security find-generic-password -a "$acct" -s "$LAZAR_KC_SERVICE" >/dev/null 2>&1
}

kc_delete() {
  local acct="$1"
  security delete-generic-password -a "$acct" -s "$LAZAR_KC_SERVICE" >/dev/null 2>&1 || true
}

# Load a value from env if set, else Keychain. Prints to stdout.
kc_resolve() {
  local acct="$1"
  local from_env
  from_env=$(printenv "$acct" 2>/dev/null || true)
  if [ -n "${from_env:-}" ]; then
    printf '%s' "$from_env"
    return 0
  fi
  kc_get "$acct"
}
