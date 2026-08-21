#!/usr/bin/env bash
# Probe each Keychain-backed API key with a minimal live call.
# Prints PASS/FAIL only — never secret values or response bodies with keys.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/keychain-common.sh"
PASS=0; FAIL=0; MISS=0
ok(){ echo "  [PASS] $1"; PASS=$((PASS+1)); }
no(){ echo "  [FAIL] $1 — $2"; FAIL=$((FAIL+1)); }
ms(){ echo "  [MISS] $1"; MISS=$((MISS+1)); }
# HTTP helper: status code only. Args after URL are curl args (headers etc).
http_code() {
  local url="$1"; shift
  curl -sS -o /tmp/lazar-kc-verify-body.$$ -w '%{http_code}' --max-time 20 "$@" "$url" 2>/dev/null \
    || echo "000"
}
# probe_messages NAME BASE_URL MODEL — one tiny Anthropic-Messages call against
# the provider that actually owns the key. Never against a proxy or another
# provider's base: a key is valid only where it was issued.
probe_messages() {
  local name="$1" base="$2" model="$3" val code
  if ! val=$(kc_get "$name" 2>/dev/null); then ms "$name"; return; fi
  code=$(http_code "${base%/}/v1/messages" \
    -H "x-api-key: $val" \
    -H "anthropic-version: 2023-06-01" \
    -H "content-type: application/json" \
    -d "{\"model\":\"$model\",\"max_tokens\":16,\"messages\":[{\"role\":\"user\",\"content\":\"ping\"}]}")
  case "$code" in
    200) ok "$name ($model @ $base, HTTP $code)" ;;
    401|403) no "$name" "auth rejected HTTP $code at $base" ;;
    *) no "$name" "HTTP $code at $base" ;;
  esac
}

echo "Verifying Keychain keys (service=$LAZAR_KC_SERVICE)"
echo "────────────────────────────────────────"

# --- Cross-provider duplicates: the same secret stored under two names means a
# proxy alias (e.g. ANTHROPIC_API_KEY="$MINIMAX_API_KEY") was persisted. Every
# request to the provider that did NOT issue the key will 401.
declare -a _kc_seen_names=() _kc_seen_vals=()
for _name in "${LAZAR_KEY_NAMES[@]}"; do
  _val=$(kc_get "$_name" 2>/dev/null) || continue
  for _i in "${!_kc_seen_vals[@]}"; do
    if [ "${_kc_seen_vals[$_i]}" = "$_val" ]; then
      no "$_name" "identical to ${_kc_seen_names[$_i]} — one of them is the wrong provider's key; re-store the real one"
    fi
  done
  _kc_seen_names+=("$_name"); _kc_seen_vals+=("$_val")
done
unset _kc_seen_names _kc_seen_vals _name _val _i

probe_messages ANTHROPIC_API_KEY   "https://api.anthropic.com"        "claude-haiku-4-5"
probe_messages MINIMAX_API_KEY     "https://api.minimax.io/anthropic" "MiniMax-M3"
probe_messages KIMI_API_KEY        "https://api.moonshot.ai/anthropic" "kimi-k3[1m]"
probe_messages KIMI_CODING_API_KEY "https://api.kimi.com/coding"      "kimi-k3"
probe_messages OPENROUTER_API_KEY  "https://openrouter.ai/api"        "anthropic/claude-haiku-4.5"
rm -f /tmp/lazar-kc-verify-body.$$ 2>/dev/null

echo "────────────────────────────────────────"
echo "PASS=$PASS  FAIL=$FAIL  MISS=$MISS"
[ "$FAIL" -eq 0 ]
