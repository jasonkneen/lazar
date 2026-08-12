# Provider protocol

**There is no lazar-side HTTP server.** The "API" surface of lazar is:

1. The CLI — [`bin/lazar`](../cli/lazar.md) and [`bin/lazar-spawn`](../cli/lazar-spawn.md).
2. The structured [`--output-format stream-json`](output-formats.md) event stream.
3. The **underlying provider's HTTP endpoint**, which the kernel speaks to
   directly — and which power users can hit with `curl` just the same.

This page documents (3).

## The protocol: Anthropic Messages

The kernel is an Anthropic-protocol client. It makes SSE-streamed `POST`
requests to `<base>/v1/messages` using `reqwest`, with:

- `x-api-key: <key>` — the API key header
- `anthropic-version` — the standard Messages API version header
- one tool declared: `bash(command)`

Power users can talk to the same endpoint directly — nothing about lazar's
side of the wire is proprietary.

## Endpoint resolution

From `src/src/main.rs`:

| Condition | Base URL | Default model |
|---|---|---|
| `ANTHROPIC_BASE_URL` set | `<your value>` (kernel appends `/v1/messages`) | `claude-sonnet-4-6` |
| Only `MINIMAX_API_KEY` set (MiniMax fallback) | `https://api.minimax.io/anthropic` | `MiniMax-M3` |
| Otherwise | `https://api.anthropic.com` | `claude-sonnet-4-6` |

Key resolution: `ANTHROPIC_API_KEY` first, then `MINIMAX_API_KEY`. If neither
is set the kernel aborts before any network call.

## Known-good endpoints (from `memory/PINNED.md`)

- **MiniMax (default for this install):** native Anthropic protocol at
  `https://api.minimax.io/anthropic` — no proxy. Default model `MiniMax-M3`.
  Key under `MINIMAX_API_KEY` in `~/lazar/.env` (mode 600).
- **OpenRouter:** exposes an Anthropic-protocol endpoint at
  `https://openrouter.ai/api/v1/messages` that the kernel speaks with no
  proxy. Routed by the launcher when `LAZAR_BACKEND=openrouter`
  (`ANTHROPIC_BASE_URL=https://openrouter.ai/api`).
- **Kimi / Moonshot:** `https://api.moonshot.ai/anthropic` via the
  `LAZAR_BACKEND=kimi` launcher preset.

`LAZAR_BACKEND` is a **launcher-level** preset selector (used by
`workspace/lazar-env.sh` and `lazartui.sh`); the kernel itself only reads
`ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY` / `MINIMAX_API_KEY`, and
`LAZAR_MODEL`. See [Configuration](../getting-started/configuration.md).

## Vision

The MiniMax `/v1/messages` endpoint accepts `image_url` blocks, so the agent
can see images (see `skills/vision/`). OpenRouter's Anthropic-compat endpoint
passes image blocks through unchanged.

## Listing models (MiniMax)

`GET https://api.minimax.io/v1/models` returns the MiniMax model list. The
TUI's model picker reads `~/lazar/workspace/tui/models.txt` (plus a small
built-in fallback) as its source of truth.

## Secret isolation

Tool subprocesses do **not** inherit `ANTHROPIC_API_KEY` by default — nested
`lazar -p` calls from inside agent bash fail without a key unless you opt in
with `LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY=1`. When the sandbox is off, a set
of `PASSTHROUGH_VARS` (SSH agent, GitHub/GitLab tokens, cloud credentials,
etc. — see [Environment variables](env-vars.md)) *is* forwarded so operator
CLIs keep working.
