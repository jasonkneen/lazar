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

## Provider catalog and endpoint resolution

`~/lazar/providers.toml` is the operator-owned provider catalog, seeded from
`src/providers.toml`. Each entry names its endpoint, key environment variable,
default model, and model list. Inspect it without exposing secrets:

```bash
lazar providers
lazar providers --json
```

`--provider <id>` or `LAZAR_PROVIDER` selects an entry. Its endpoint and named
key are authoritative, preventing stale generic Anthropic variables from routing
a selected model to the wrong provider. The kernel reads the named environment
variable first, then the macOS Keychain (`service=lazar`, account equal to the
variable name) at recursion depth zero. `--model`, then `LAZAR_MODEL`, overrides
the entry's default model.

Without provider selection, the legacy resolution remains available:
`ANTHROPIC_BASE_URL` / `ANTHROPIC_API_KEY`, then the MiniMax fallback, then the
Anthropic default. A missing key aborts before any network call.

## Bundled catalog entries

- **Anthropic:** `https://api.anthropic.com`; key `ANTHROPIC_API_KEY`.
- **MiniMax:** `https://api.minimax.io/anthropic`; key `MINIMAX_API_KEY`;
  default model `MiniMax-M3`.
- **OpenRouter:** exposes an Anthropic-protocol endpoint at
  `https://openrouter.ai/api/v1/messages` that the kernel speaks with no
  proxy; key `OPENROUTER_API_KEY`.
- **Kimi / Moonshot:** `https://api.moonshot.ai/anthropic`; key
  `KIMI_API_KEY`.
- **Kimi Code:** `https://api.kimi.com/coding`; key `KIMI_CODING_API_KEY`.

Full model lists are not duplicated here. Check the live catalog with `lazar
providers`; edit `providers.toml` when an operator needs to update one.

## Vision

The MiniMax `/v1/messages` endpoint accepts `image_url` blocks, so the agent
can see images (see `skills/vision/`). OpenRouter's Anthropic-compat endpoint
passes image blocks through unchanged.

## Models and the TUI

The native Rust TUI gets providers and model lists from `lazar providers
--json`; it carries no endpoint table or secret handling of its own. Ctrl+P and
Ctrl+O open provider and model pickers. Switching restarts the long-lived ACP
child and reloads the same Lazar session.

## Secret isolation

Tool subprocesses do **not** inherit the active provider key by default. To let
nested Lazar calls authenticate, start the parent with
`LAZAR_TOOL_INHERIT_PROVIDER_KEY=1`; only the selected provider's named key is
exposed. The old `LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY` name is a compatibility
alias. When the sandbox is off, a set
of `PASSTHROUGH_VARS` (SSH agent, GitHub/GitLab tokens, cloud credentials,
etc. — see [Environment variables](env-vars.md)) *is* forwarded so operator
CLIs keep working.
