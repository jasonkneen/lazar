# Configuration

lazar keeps configuration deliberately thin: a provider catalog, environment
variables or Keychain items for secrets, and the model choice.

## API keys

The kernel reads two key variables, in this order:

1. `ANTHROPIC_API_KEY` — the primary key, sent as the `x-api-key` header.
2. `MINIMAX_API_KEY` — fallback. When only this is set, the kernel
   automatically targets `https://api.minimax.io/anthropic` and defaults the
   model to `MiniMax-M3`.

If neither is set, the kernel aborts with
`ANTHROPIC_API_KEY is not set (nor MINIMAX_API_KEY)`.

Prefer the macOS Keychain via the `_meta/env-vars` skill (service name `lazar`,
overridable via `LAZAR_KC_SERVICE`). A mode-600 `~/lazar/.env` remains an
explicit migration source for the skill; the kernel does not source it.
Never commit secrets.

## Provider and endpoint selection

Use `--provider <id>` or `LAZAR_PROVIDER` to select an entry from
`$LAZAR_HOME/providers.toml`. Run `lazar providers` to see ids, models, and
whether each entry's key is available. The catalog entry names the environment
variable used for its key; depth-zero processes also check the macOS Keychain
with service `lazar` and that variable name as the account.

- `ANTHROPIC_BASE_URL` — override the API base. The kernel appends
  `/v1/messages` to whatever you set. Default: `https://api.anthropic.com`
  (or the MiniMax URL when in MiniMax-fallback mode).
Examples:

```bash
lazar --provider minimax -p "Inspect this repository"
LAZAR_PROVIDER=openrouter lazar -p "Inspect this repository"
lazartui --provider kimi
```

## Model selection

Precedence (first wins):

1. `--model <MODEL>` CLI flag
2. `LAZAR_MODEL` environment variable
3. Selected provider's `default_model`
4. Built-in default: `claude-sonnet-4-6`
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
| `LAZAR_MAX_TOKENS` | 8192 | Model output cap per turn; values above the compiled ceiling are clamped |
| `LAZAR_TOOL_TIMEOUT_SECS` | 120 | Wall-clock timeout per bash tool call |
| `LAZAR_NO_SANDBOX` | unset | Set to `1` to disable sandbox-exec |
