# Configuration

lazar keeps configuration deliberately thin: a handful of environment
variables, an optional `.env` file, and the model choice.

## API keys

The kernel reads two key variables, in this order:

1. `ANTHROPIC_API_KEY` — the primary key, sent as the `x-api-key` header.
2. `MINIMAX_API_KEY` — fallback. When only this is set, the kernel
   automatically targets `https://api.minimax.io/anthropic` and defaults the
   model to `MiniMax-M3`.

If neither is set, the kernel aborts with
`ANTHROPIC_API_KEY is not set (nor MINIMAX_API_KEY)`.

Keys live in `~/lazar/.env` (mode 600), sourceable by the launcher, or in the
macOS Keychain via the `env-vars` skill (service name `lazar`, overridable via
`LAZAR_KC_SERVICE`). Never commit secrets.

## Endpoint and backend selection

- `ANTHROPIC_BASE_URL` — override the API base. The kernel appends
  `/v1/messages` to whatever you set. Default: `https://api.anthropic.com`
  (or the MiniMax URL when in MiniMax-fallback mode).
- `LAZAR_BACKEND` — **not read by the kernel**. It is a *launcher-level*
  preset selector used by `workspace/lazar-env.sh` and `lazartui.sh` to set
  `ANTHROPIC_BASE_URL` / key / model for you. Known presets include
  `minimax` (default), `kimi`, `openrouter`, `anthropic`, and `proxy`.

The launcher contract mirrors Claude Code:

| Claude Code | lazar kernel |
|---|---|
| `ANTHROPIC_BASE_URL` | `ANTHROPIC_BASE_URL` |
| `ANTHROPIC_AUTH_TOKEN` | mapped to `ANTHROPIC_API_KEY` (sent as `x-api-key`) |
| `ANTHROPIC_MODEL` | mapped to `LAZAR_MODEL` |

Usage:

```bash
LAZAR_BACKEND=minimax    source ~/lazar/workspace/lazar-env.sh
LAZAR_BACKEND=openrouter source ~/lazar/workspace/lazar-env.sh
LAZAR_BACKEND=kimi       source ~/lazar/workspace/lazar-env.sh
```

## Model selection

Precedence (first wins):

1. `--model <MODEL>` CLI flag
2. `LAZAR_MODEL` environment variable
3. Built-in default: `claude-sonnet-4-6`
   (`MiniMax-M3` when running in MiniMax-fallback mode)

## Runtime root

`LAZAR_HOME` overrides the install/runtime root (default `~/lazar`). All
skills, memory, workspace, logs, and hooks resolve relative to it.

## Tuning knobs

Every operational limit is an env var — see the full
[environment variable reference](../api/env-vars.md). The most commonly tuned:

| Variable | Default | Purpose |
|---|---|---|
| `LAZAR_MAX_TURNS` | 50 | Max agent loop turns per invocation |
| `LAZAR_MAX_TOOL_CALLS` | 200 | Max tool calls per invocation |
| `LAZAR_MAX_TOKENS` | 16384 | Model output cap per turn |
| `LAZAR_TOOL_TIMEOUT_SECS` | 120 | Wall-clock timeout per bash tool call |
| `LAZAR_NO_SANDBOX` | unset | Set to `1` to disable sandbox-exec |
