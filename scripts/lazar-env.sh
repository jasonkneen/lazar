#!/usr/bin/env bash
# lazar-env.sh — single source of truth for keys + provider defaults.
#
# Source from wrappers (lazar / glazar / lazartui):
#   source "$LAZAR_HOME/scripts/lazar-env.sh"
#
# Defaults: MiniMax account + MiniMax-M3 via the *native* Anthropic endpoint
# (no local proxy). Proxy is only for providers that speak OpenAI-shaped APIs.
#
# Backends (LAZAR_BACKEND):
#   minimax      (default) — direct https://api.minimax.io/anthropic
#   openrouter             — direct OpenRouter Anthropic-compatible API
#   xai | fireworks | nvidia | gemini | zen
#                          — local proxy.py on PROXY_PORT (default 4000)
#   proxy                  — assume proxy already running; point BASE_URL at it
#
# Overrides (already-set env wins):
#   LAZAR_BACKEND, LAZAR_MODEL, ANTHROPIC_BASE_URL, ANTHROPIC_API_KEY, PROXY_PORT
#
# Never prints secret values.

# Intentionally NOT set -e/-u: this file is sourced into interactive shells.
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
export LAZAR_HOME

# ── 1. Secrets from Keychain (+ non-secrets from .env) ──────────────────────
_LOAD="$LAZAR_HOME/skills/env-vars/load-all.sh"
if [ -f "$_LOAD" ]; then
  # shellcheck disable=SC1090
  source "$_LOAD" 2>/dev/null || true
else
  # fallback: .env non-secrets only
  if [ -f "$LAZAR_HOME/.env" ]; then
    set -a
    # shellcheck disable=SC1090
    source "$LAZAR_HOME/.env"
    set +a
  fi
  if [ -f "$LAZAR_HOME/workspace/provider-switch/provider-env.sh" ]; then
    # shellcheck disable=SC1090
    source "$LAZAR_HOME/workspace/provider-switch/provider-env.sh" 2>/dev/null || true
  fi
fi

# ── 2. Backend selection ────────────────────────────────────────────────────
BACKEND="${LAZAR_BACKEND:-minimax}"
PROXY_DIR="$LAZAR_HOME/workspace/provider-switch"
PROXY_PORT="${PROXY_PORT:-4000}"
export LAZAR_USE_PROXY="${LAZAR_USE_PROXY:-0}"

_start_proxy() {
  local provider="$1" model="$2"
  local start="$PROXY_DIR/start-proxy.sh"
  if [ ! -x "$start" ]; then
    echo "[lazar-env] WARN: missing $start — cannot start proxy for $provider" >&2
    return 1
  fi
  # start-proxy.sh already kills stale pid and loads Keychain via provider-env
  PROVIDER="$provider" MODEL="$model" PROXY_PORT="$PROXY_PORT" \
    bash "$start" "$provider" "$model" >/dev/null
}

case "$BACKEND" in
  minimax|"")
    # Native Anthropic protocol — no proxy.
    export ANTHROPIC_BASE_URL="${ANTHROPIC_BASE_URL:-https://api.minimax.io/anthropic}"
    if [ -z "${ANTHROPIC_API_KEY:-}" ] && [ -n "${MINIMAX_API_KEY:-}" ]; then
      export ANTHROPIC_API_KEY="$MINIMAX_API_KEY"
    fi
    export LAZAR_MODEL="${LAZAR_MODEL:-MiniMax-M3}"
    export LAZAR_USE_PROXY=0
    export ACTIVE_PROVIDER_NAME="minimax"
    ;;

  openrouter)
    export ANTHROPIC_BASE_URL="${ANTHROPIC_BASE_URL:-https://openrouter.ai/api}"
    if [ -n "${OPENROUTER_API_KEY:-}" ]; then
      export ANTHROPIC_API_KEY="$OPENROUTER_API_KEY"
    fi
    export LAZAR_MODEL="${LAZAR_MODEL:-openrouter/owl-alpha}"
    export LAZAR_USE_PROXY=0
    export ACTIVE_PROVIDER_NAME="openrouter"
    ;;

  xai|fireworks|nvidia|gemini|zen)
    # OpenAI-shaped upstreams → local Anthropic↔OpenAI proxy.
    local_model_default=""
    case "$BACKEND" in
      xai)       local_model_default="grok-3-mini" ;;
      fireworks) local_model_default="accounts/fireworks/models/deepseek-v4-pro" ;;
      nvidia)    local_model_default="meta/llama-3.3-70b-instruct" ;;
      gemini)    local_model_default="gemini-2.0-flash" ;;
      zen)       local_model_default="claude-sonnet-4" ;;
    esac
    export LAZAR_MODEL="${LAZAR_MODEL:-$local_model_default}"
    if ! _start_proxy "$BACKEND" "$LAZAR_MODEL"; then
      echo "[lazar-env] proxy start failed for backend=$BACKEND" >&2
    fi
    export ANTHROPIC_BASE_URL="http://127.0.0.1:${PROXY_PORT}"
    export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-proxy-local}"
    export LAZAR_USE_PROXY=1
    export ACTIVE_PROVIDER_NAME="$BACKEND"
    ;;

  proxy)
    # Caller manages proxy; we only point at it.
    export ANTHROPIC_BASE_URL="${ANTHROPIC_BASE_URL:-http://127.0.0.1:${PROXY_PORT}}"
    export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-proxy-local}"
    export LAZAR_MODEL="${LAZAR_MODEL:-MiniMax-M3}"
    export LAZAR_USE_PROXY=1
    export ACTIVE_PROVIDER_NAME="proxy"
    ;;

  *)
    echo "[lazar-env] unknown LAZAR_BACKEND='$BACKEND' (minimax|openrouter|xai|fireworks|nvidia|gemini|zen|proxy)" >&2
    return 2 2>/dev/null || exit 2
    ;;
esac

# Persist default model for TUI / lazar-model readers (only if still defaulting to MiniMax)
_model_file="${LAZAR_MEMORY:-$LAZAR_HOME/memory}/model.txt"
if [ "$BACKEND" = "minimax" ] || [ -z "${BACKEND:-}" ]; then
  mkdir -p "$(dirname "$_model_file")"
  # Only rewrite if empty or clearly not a MiniMax model and user didn't set LAZAR_MODEL explicitly
  if [ ! -f "$_model_file" ] || ! rg -q 'MiniMax' "$_model_file" 2>/dev/null; then
    if [ -z "${LAZAR_MODEL_SET_BY_USER:-}" ]; then
      printf '%s\n' "${LAZAR_MODEL:-MiniMax-M3}" > "$_model_file"
    fi
  fi
fi

# Quiet status for interactive shells (skip in -p wrappers that don't want noise)
if [ "${LAZAR_ENV_QUIET:-0}" != "1" ] && [ -t 2 ]; then
  echo "[lazar-env] backend=$BACKEND model=${LAZAR_MODEL:-?} base=${ANTHROPIC_BASE_URL:-?} proxy=${LAZAR_USE_PROXY}" >&2
fi
