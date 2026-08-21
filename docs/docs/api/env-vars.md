# Environment variables

This reference separates variables read by the kernel from variables consumed
by bundled helper scripts. Provider key names come from the checked-in
`providers.toml`; the catalog remains the authority.

## Provider / auth

| Variable | Purpose |
|---|---|
| `LAZAR_PROVIDER` | Select an entry of `$LAZAR_HOME/providers.toml` (same as `--provider`). Authoritative for endpoint and key; the key is read from the entry's env var, else the macOS Keychain (`service=$LAZAR_KC_SERVICE`, default `lazar`) at depth 0. `lazar providers [--json]` lists the catalog. |
| `LAZAR_KC_SERVICE` | Keychain service name the kernel (and `_meta/env-vars`) use for provider keys; default `lazar`. |
| `ANTHROPIC_API_KEY` | API key sent as `x-api-key` to the provider endpoint when no provider is selected. Falls back to `MINIMAX_API_KEY` when unset. |
| `ANTHROPIC_BASE_URL` | Override the API endpoint (e.g. `https://api.minimax.io/anthropic`, `https://openrouter.ai/api`). |
| `MINIMAX_API_KEY` | Fallback key when `ANTHROPIC_API_KEY` is unset; the kernel then defaults to `https://api.minimax.io/anthropic` and model `MiniMax-M3`. |
| `KIMI_API_KEY` / `KIMI_CODING_API_KEY` / `OPENROUTER_API_KEY` | Catalog-owned key names for the bundled Kimi, Kimi Code, and OpenRouter entries. Selected entries read their named environment variable before Keychain. |

## Model / request

| Variable | Purpose |
|---|---|
| `LAZAR_MODEL` | Model id for the invocation (overrides the built-in default). |
| `LAZAR_MAX_TOKENS` | `max_tokens` sent on every API call (default 16384). A reply that stops at this limit is continued automatically, not failed. |
| `LAZAR_CONTEXT_WINDOW` | Model context window in tokens (overrides the provider's `context_window` in `providers.toml`; default 200000). The loop compacts before a request would exceed ~80% of it. |
| `LAZAR_MAX_TURNS` / `LAZAR_MAX_TOOL_CALLS` | Hard ceilings on agent-loop turns and tool calls per invocation. Nested tool commands inherit these, plus `LAZAR_MAX_TOKENS`. |
| `LAZAR_STREAM_IDLE_SECS` | How long a provider response may go without a content frame before the kernel fails the turn (default 120). Comment and keepalive lines do not reset it. |

## Paths

| Variable | Purpose |
|---|---|
| `LAZAR_HOME` | The harness root (default `~/lazar`). Everything keys off this. |
| `LAZAR_LOGS` / `LAZAR_MEMORY` / `LAZAR_SKILLS` / `LAZAR_WORKSPACE` | Exported by the kernel into every tool and hook subprocess as absolute paths under `LAZAR_HOME`. They are outputs, not overrides: the kernel never reads them back. |
| `LAZAR_BIN` | Kernel binary override used by `lazar-spawn`, `lazar-failover`, and the native TUI provider probe (default `$LAZAR_HOME/bin/lazar`). |
| `LAZAR_INHERITED_CWD` / `LAZAR_INHERITED_ADD_DIRS` | Cwd and extra allowed directories passed down to a spawned child. |

## Logging / sessions

| Variable | Purpose |
|---|---|
| `LAZAR_LOG_MAX_BYTES` | Rotation threshold for `logs/stream.jsonl` (default ~10MB). |
| `LAZAR_LOG_ARCHIVE_KEEP` | How many rotated archives to keep. |
| `LAZAR_SESSION_ID` | Current session id; set by the kernel, consumed by hooks and the TUI. |
| `LAZAR_SESSION_HISTORY_MAX_BYTES` | Cap on prior-session history prepended by `--session` / `--resume` (default: half the context window's byte budget). Any positive value is honored; set it lower for models whose tool-calling degrades on long replays. |

## Tools / sandbox

| Variable | Purpose |
|---|---|
| `LAZAR_SANDBOX` | `1` enables `sandbox-exec`. The sandbox is off when this is unset. |
| `LAZAR_NO_SANDBOX` | `1` forces the sandbox off for the invocation. |
| `LAZAR_GUARDRAILS` | One-build override read by `scripts/kernel-build.sh`, not the kernel. `on` relocks and `off` leaves `bin/` and `src/` writable. Without it, the builder follows operator-owned `$LAZAR_HOME/.guardrails`; that persisted mode is also read when the next kernel process starts. |
| `LAZAR_TOOL_TIMEOUT_SECS` | Wall-clock timeout per bash tool call. |
| `LAZAR_TOOL_OUTPUT_MAX_BYTES` | Cap on captured tool output returned to the model (default 32000 bytes, up to 256000). Older outputs are elided further by in-turn compaction. |
| `LAZAR_TOOL_ENV` | Set to `1` on every tool subprocess. A nested `lazar` only trusts `LAZAR_INHERITED_CWD` / `LAZAR_INHERITED_ADD_DIRS` when this marker is present. |
| `LAZAR_TOOL_INHERIT_PROVIDER_KEY` | Operator opt-in on the parent Lazar process. `1` exposes only the active catalog provider's named key to tool subprocesses so nested Lazar calls can authenticate. Default is isolation. |
| `LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY` | Compatibility alias for `LAZAR_TOOL_INHERIT_PROVIDER_KEY`. Despite its old name, it follows the active provider. |

## Hooks

| Variable | Purpose |
|---|---|
| `LAZAR_HOOK_EVENT` | Name of the lifecycle event being fired (`session-start`, `user-prompt`, `pre-tool`, `post-tool`, `session-end`, `log-rotation`, `agent-stop`, `tick`). |
| `LAZAR_HOOK_PAYLOAD_LEN` | Byte length of the JSON payload handed to the hook. |
| `LAZAR_HOOK_TIMEOUT_SECS` | Per-hook wall-clock timeout. |
| `LAZAR_STRICT_PRE_TOOL_HOOKS` | Strict mode for pre-tool hooks (a failing/vetoing hook blocks the tool call). |

## Sub-agents

| Variable | Purpose |
|---|---|
| `LAZAR_DEPTH` | Recursion depth of the current invocation (cap is the kernel's `MAX_DEPTH`, 5). |
| `LAZAR_SPAWN_BUDGET` | Max spawns per session (default 10). |
| `LAZAR_SPAWN_CEILING` | Max retries per task (default 2). |
| `LAZAR_SPAWN_TIMEOUT` | Wall-clock kill for a wedged child (default 300s). |
| `LAZAR_SUBAGENT_STATE` | State dir for spawn bookkeeping. Only honored together with `LAZAR_SPAWN_TEST=1`; production state lives in `workspace/subagent/` (logs/ is not writable from the sandbox). |
| `LAZAR_SPAWN_TEST` / `LAZAR_TEST_FLAG_*` | Test hooks used by `skills/_meta/subagent/selftest.sh` and kernel tests. |

## Bundled helper scripts

| Variable | Purpose |
|---|---|
| `LAZAR_FAILOVER_BIN` | Test/override hook for the bundled provider-failover skill's executable. |
| `HARNESS_EVALS_PROVIDER` / `HARNESS_EVALS_REPEAT` | Default provider id and repeat count for the bundled harness-evals runner. |
| `HARNESS_EVALS_RESULTS` / `HARNESS_EVALS_SKILL_DIR` / `HARNESS_EVALS_PYTHON` | Override harness-evals result, skill, and Python paths. |
| `HARNESS_EVALS_CMD` / `HARNESS_EVALS_STREAM_FIXTURE` | Inputs for the harness-evals command and offline stream adapters. |

!!! note
    Rows marked script- or skill-level are not read by the kernel
    binary. For everything else, `grep -rn 'VAR_NAME' ~/lazar/src/src` is the
    ground truth — check it before documenting a new variable.
