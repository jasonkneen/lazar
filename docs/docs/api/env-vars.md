# Environment variables

Every `LAZAR_*` variable below was enumerated by grepping `src/` and
`bin/lazar` (`grep -rhoE 'LAZAR_[A-Z_]+' src bin | sort -u`) — this list is
ground truth from the binary, not from memory. Provider keys
(`ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`, `MINIMAX_API_KEY`,
`OPENROUTER_API_KEY`) come from the same sweep plus `memory/PINNED.md`.

## Provider / auth

| Variable | Purpose |
|---|---|
| `ANTHROPIC_API_KEY` | API key sent as `x-api-key` / `Authorization: Bearer` to the provider endpoint. |
| `ANTHROPIC_BASE_URL` | Override the API endpoint (e.g. `https://api.minimax.io/anthropic`, `https://openrouter.ai/api`). |
| `MINIMAX_API_KEY` | MiniMax key, stored in `~/lazar/.env`; the launcher maps it into the request auth. |
| `LAZAR_ENV_OWNED_API_KEY` / `LAZAR_ENV_OWNED_AUTH_TOKEN` / `LAZAR_ENV_OWNED_BASE_URL` | Mark which credential vars the kernel itself set (so it doesn't leak or double-override them in children). |
| `LAZAR_KC_SERVICE` / `LAZAR_KEY_NAMES` | Keychain service name and the list of key names the kernel knows how to fetch. |

## Model / request

| Variable | Purpose |
|---|---|
| `LAZAR_MODEL` | Model id for the invocation (overrides the built-in default). |
| `LAZAR_MAX_TOKENS` | Response cap per API call (default 8192 — exceeding it in one write is the classic big-write truncation abort). |
| `LAZAR_MAX_TURNS` / `LAZAR_MAX_TOOL_CALLS` | Hard ceilings on agent-loop turns and tool calls per invocation. |
| `LAZAR_STALL_NUDGES` | Bounded stall-recovery nudges per invocation (default 2). Fires only when the final sentence narrates unfinished intent; never on questions, never unbounded. |
| `LAZAR_GROUNDING_RETRIES` | How many correction turns the kernel allows after a failed `[VERIFY]` contract before returning `grounding_failed`. |

## Paths

| Variable | Purpose |
|---|---|
| `LAZAR_HOME` | The harness root (default `~/lazar`). Everything keys off this. |
| `LAZAR_BIN` / `LAZAR_LOGS` / `LAZAR_MEMORY` / `LAZAR_SKILLS` / `LAZAR_WORKSPACE` | Per-directory overrides for `bin/`, `logs/`, `memory/`, `skills/`, `workspace/`. |
| `LAZAR_INHERITED_CWD` / `LAZAR_INHERITED_ADD_DIRS` | Cwd and extra allowed directories passed down to a spawned child. |

## Logging / sessions

| Variable | Purpose |
|---|---|
| `LAZAR_LOG_MAX_BYTES` | Rotation threshold for `logs/stream.jsonl` (default ~10MB). |
| `LAZAR_LOG_ARCHIVE_KEEP` | How many rotated archives to keep. |
| `LAZAR_SESSION_ID` | Current session id; set by the kernel, consumed by hooks and the TUI. |
| `LAZAR_SESSION_HISTORY_MAX_BYTES` | Cap on how much prior-session history is prepended on `--resume`. |
| `LAZAR_ENV_QUIET` | Suppress non-essential env/banner output. |

## Tools / sandbox

| Variable | Purpose |
|---|---|
| `LAZAR_NO_SANDBOX` | `1` disables the `sandbox-exec` profile for the invocation (full operator filesystem access). |
| `LAZAR_GUARDRAILS` | Guardrails mode (`on`/`off`); toggled by the `guardrails` skill via `scripts/lazar-guardrails.sh`. |
| `LAZAR_TOOL_TIMEOUT_SECS` | Wall-clock timeout per bash tool call. |
| `LAZAR_TOOL_OUTPUT_MAX_BYTES` | Cap on captured tool output returned to the model. |
| `LAZAR_TOOL_ENV` | Extra `KEY=VAL` entries injected into tool environments. |
| `LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY` | `1` lets a spawned child inherit the parent's API key. **Default is isolation** — children spawned without this fail auth with `401 invalid x-api-key`. |
| `LAZAR_BASH_FALLBACK` | Allow bash fallback behavior when the primary tool path is unavailable. |

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
| `LAZAR_SUBAGENT_STATE` | State dir for spawn bookkeeping (`logs/subagent/`). |
| `LAZAR_SPAWN_TEST` / `LAZAR_TEST_FLAG_*` | Test hooks used by `skills/_meta/subagent/selftest.sh` and kernel tests. |

!!! note
    If a variable isn't in this table, it isn't read by the kernel. Verify
    with `grep -r 'VAR_NAME' ~/lazar/src` before documenting a new one.
