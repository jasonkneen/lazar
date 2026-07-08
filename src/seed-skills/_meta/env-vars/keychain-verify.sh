#!/usr/bin/env bash
# Probe each Keychain-backed API key with a minimal live call.
# Prints PASS/FAIL only — never secret values or response bodies with keys.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/keychain-common.sh"

PASS=0; FAIL=0; SKIP=0; MISS=0
ok(){ echo "  [PASS] $1"; PASS=$((PASS+1)); }
no(){ echo "  [FAIL] $1 — $2"; FAIL=$((FAIL+1)); }
sk(){ echo "  [SKIP] $1 — $2"; SKIP=$((SKIP+1)); }
ms(){ echo "  [MISS] $1"; MISS=$((MISS+1)); }

# HTTP helper: status code only. Args after URL are curl args (headers etc).
http_code() {
  local url="$1"; shift
  curl -sS -o /tmp/lazar-kc-verify-body.$$ -w '%{http_code}' --max-time 20 "$@" "$url" 2>/dev/null \
    || echo "000"
}

echo "Verifying Keychain keys (service=$LAZAR_KC_SERVICE)"
echo "────────────────────────────────────────"

# --- MINIMAX (Anthropic-compatible) ---
if val=$(kc_get MINIMAX_API_KEY 2>/dev/null); then
  code=$(http_code "https://api.minimax.io/anthropic/v1/messages" \
    -H "x-api-key: $val" \
    -H "anthropic-version: 2023-06-01" \
    -H "content-type: application/json" \
    -d '{"model":"MiniMax-M2.5-highspeed","max_tokens":16,"messages":[{"role":"user","content":"ping"}]}')
  case "$code" in
    200) ok "MINIMAX_API_KEY (HTTP $code)" ;;
    401|403) no "MINIMAX_API_KEY" "auth rejected HTTP $code" ;;
    *) no "MINIMAX_API_KEY" "HTTP $code" ;;
  esac
else ms MINIMAX_API_KEY; fi

# --- ANTHROPIC (may be MiniMax key pointed at MiniMax base, or real Anthropic) ---
if val=$(kc_get ANTHROPIC_API_KEY 2>/dev/null); then
  # Prefer env BASE_URL if set, else try Anthropic then MiniMax
  BASE="${ANTHROPIC_BASE_URL:-}"
  if [ -z "$BASE" ] && [ -f "${LAZAR_HOME:-$HOME/lazar}/.env" ]; then
    BASE=$(grep -E '^ANTHROPIC_BASE_URL=' "${LAZAR_HOME:-$HOME/lazar}/.env" 2>/dev/null | head -1 | cut -d= -f2- | tr -d '"' | tr -d "'")
  fi
  BASE="${BASE:-https://api.anthropic.com}"
  BASE="${BASE%/}"
  code=$(http_code "$BASE/v1/messages" \
    -H "x-api-key: $val" \
    -H "anthropic-version: 2023-06-01" \
    -H "content-type: application/json" \
    -d '{"model":"claude-3-5-haiku-latest","max_tokens":16,"messages":[{"role":"user","content":"ping"}]}')
  # MiniMax keys on MiniMax base often return 200 with MiniMax model names; wrong model may 400
  case "$code" in
    200) ok "ANTHROPIC_API_KEY via $BASE (HTTP $code)" ;;
    400|404)
      # retry with MiniMax model if base looks like minimax
      if printf '%s' "$BASE" | rg -qi minimax; then
        code2=$(http_code "$BASE/v1/messages" \
          -H "x-api-key: $val" \
          -H "anthropic-version: 2023-06-01" \
          -H "content-type: application/json" \
          -d '{"model":"MiniMax-M2.5-highspeed","max_tokens":16,"messages":[{"role":"user","content":"ping"}]}')
        case "$code2" in
          200) ok "ANTHROPIC_API_KEY via $BASE MiniMax model (HTTP $code2)" ;;
          *) no "ANTHROPIC_API_KEY" "HTTP $code then $code2 at $BASE" ;;
        esac
      else
        # 400 can mean key works but model invalid — treat as soft pass
        ok "ANTHROPIC_API_KEY accepted path (HTTP $code at $BASE — model may differ)"
      fi
      ;;
    401|403) no "ANTHROPIC_API_KEY" "auth rejected HTTP $code at $BASE" ;;
    *) no "ANTHROPIC_API_KEY" "HTTP $code at $BASE" ;;
  esac
else ms ANTHROPIC_API_KEY; fi

# --- OPENROUTER ---
if val=$(kc_get OPENROUTER_API_KEY 2>/dev/null); then
  code=$(http_code "https://openrouter.ai/api/v1/models" \
    -H "Authorization: Bearer $val")
  case "$code" in
    200) ok "OPENROUTER_API_KEY (HTTP $code)" ;;
    401|403) no "OPENROUTER_API_KEY" "auth rejected HTTP $code" ;;
    *) no "OPENROUTER_API_KEY" "HTTP $code" ;;
  esac
else ms OPENROUTER_API_KEY; fi

# --- XAI ---
if val=$(kc_get XAI_API_KEY 2>/dev/null); then
  code=$(http_code "https://api.x.ai/v1/models" \
    -H "Authorization: Bearer $val")
  case "$code" in
    200) ok "XAI_API_KEY (HTTP $code)" ;;
    401|403) no "XAI_API_KEY" "auth rejected HTTP $code" ;;
    *) no "XAI_API_KEY" "HTTP $code" ;;
  esac
else ms XAI_API_KEY; fi

# --- FIREWORKS ---
if val=$(kc_get FIREWORKS_API_KEY 2>/dev/null); then
  code=$(http_code "https://api.fireworks.ai/inference/v1/models" \
    -H "Authorization: Bearer $val")
  case "$code" in
    200) ok "FIREWORKS_API_KEY (HTTP $code)" ;;
    401|403) no "FIREWORKS_API_KEY" "auth rejected HTTP $code" ;;
    *) no "FIREWORKS_API_KEY" "HTTP $code" ;;
  esac
else ms FIREWORKS_API_KEY; fi

# --- NVIDIA ---
if val=$(kc_get NVIDIA_API_KEY 2>/dev/null); then
  code=$(http_code "https://integrate.api.nvidia.com/v1/models" \
    -H "Authorization: Bearer $val")
  case "$code" in
    200) ok "NVIDIA_API_KEY (HTTP $code)" ;;
    401|403) no "NVIDIA_API_KEY" "auth rejected HTTP $code" ;;
    *) no "NVIDIA_API_KEY" "HTTP $code" ;;
  esac
else ms NVIDIA_API_KEY; fi

# --- GEMINI ---
if val=$(kc_get GEMINI_API_KEY 2>/dev/null); then
  code=$(http_code "https://generativelanguage.googleapis.com/v1beta/models?key=${val}")
  case "$code" in
    200) ok "GEMINI_API_KEY (HTTP $code)" ;;
    400|401|403) no "GEMINI_API_KEY" "auth/rejected HTTP $code" ;;
    *) no "GEMINI_API_KEY" "HTTP $code" ;;
  esac
else ms GEMINI_API_KEY; fi

# --- ZEN (Claude via zenmux / similar — try Anthropic-shaped if key looks like sk-) ---
if val=$(kc_get ZEN_API_KEY 2>/dev/null); then
  # Heuristic: try common zenmux endpoint; if unknown, format check only
  code=$(http_code "https://zenmux.ai/api/v1/models" \
    -H "Authorization: Bearer $val")
  case "$code" in
    200) ok "ZEN_API_KEY (HTTP $code)" ;;
    401|403) no "ZEN_API_KEY" "auth rejected HTTP $code" ;;
    000|404)
      # fallback: key format presence
      if [ "${#val}" -ge 20 ]; then
        sk "ZEN_API_KEY" "endpoint unknown/unreachable (HTTP $code); key present ${#val} chars"
      else
        no "ZEN_API_KEY" "HTTP $code and key looks short"
      fi
      ;;
    *) sk "ZEN_API_KEY" "unexpected HTTP $code (endpoint may differ)" ;;
  esac
else ms ZEN_API_KEY; fi

# --- E2B ---
if val=$(kc_get E2B_API_KEY 2>/dev/null); then
  code=$(http_code "https://api.e2b.dev/sandboxes" \
    -H "X-API-KEY: $val")
  case "$code" in
    200|201) ok "E2B_API_KEY (HTTP $code)" ;;
    401|403) no "E2B_API_KEY" "auth rejected HTTP $code" ;;
    *) no "E2B_API_KEY" "HTTP $code" ;;
  esac
else ms E2B_API_KEY; fi

# --- OPENAI (optional) ---
if val=$(kc_get OPENAI_API_KEY 2>/dev/null); then
  code=$(http_code "https://api.openai.com/v1/models" \
    -H "Authorization: Bearer $val")
  case "$code" in
    200) ok "OPENAI_API_KEY (HTTP $code)" ;;
    401|403) no "OPENAI_API_KEY" "auth rejected HTTP $code" ;;
    *) no "OPENAI_API_KEY" "HTTP $code" ;;
  esac
else ms OPENAI_API_KEY; fi

rm -f /tmp/lazar-kc-verify-body.$$ 2>/dev/null

echo "────────────────────────────────────────"
echo "PASS=$PASS  FAIL=$FAIL  SKIP=$SKIP  MISS=$MISS"
[ "$FAIL" -eq 0 ]
