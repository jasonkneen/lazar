# Sandbox

Every bash command the model issues runs through macOS `sandbox-exec` with a
profile compiled into the kernel from `src/sandbox.sb` — unless the sandbox is
explicitly off.

## What's allowed when the sandbox is ON (default)

- **Reads: open.** The profile allows `file-read*` everywhere — the agent can
  study its own kernel source and binary.
- **Writes: restricted** to the agent's writable zones:
    - `$LAZAR_HOME/skills/`
    - `$LAZAR_HOME/memory/`
    - `$LAZAR_HOME/workspace/`
    - `/tmp` (`/private/tmp`, `/private/var/tmp`, `/private/var/folders`)
    - Specific session/auth subdirs under `~/.codex` and `~/.claude`
      (`sessions/`, `todos/`, `auth.json`, etc.) so sub-agent CLIs can update
      their own state.
- **Network: open**, for API calls and skills like web search.
- **Explicitly NOT writable:** `bin/`, `src/`, `logs/` (trusted kernel-owned
  state), and operator CLI config files like `~/.codex/config.toml`,
  `~/.codex/AGENTS.md`, `~/.claude/settings.json`, `~/.claude/hooks/`,
  `~/.claude/CLAUDE.md` — a sandboxed agent writing those could inject code
  into the operator's own unsandboxed CLI sessions.

A disallowed write fails with `Operation not permitted`. The agent is expected
to learn from the failure and route around it via a skill — or ask for
guardrails to be toggled.

## The escape hatches

| Mechanism | Effect |
|---|---|
| `--no-sandbox` CLI flag | Disables sandbox-exec for this invocation |
| `LAZAR_NO_SANDBOX=1` env var | Same; propagated to nested tool subprocesses |
| `--cwd <PATH>` | Sets the working directory for every model-issued bash command (path must already exist; canonicalized before hooks/tools run) |
| `--add-dir <PATH>` | Adds an existing directory to the sandbox write allowlist; repeatable for multiple project roots |

With the sandbox off, the kernel prints a loud banner on every invocation and
the agent's bash runs with **full operator privileges** — it can read/write
anywhere the operator can, including `~/.ssh`. Only enable when you trust the
agent and the model.

Scoped project access example:

```bash
lazar --cwd ~/Documents/GitHub/my-project \
  --add-dir ~/Documents/GitHub/my-project \
  -p "inspect this project"
```

## The guardrails switch

Enforcement is a switch, not a wall. One script moves all five layers:

```bash
scripts/lazar-guardrails.sh status   # which mode am I in?
scripts/lazar-guardrails.sh off      # full operator access
scripts/lazar-guardrails.sh on       # restore enforcement
```

| # | Layer | Mechanism |
|---|-------|-----------|
| 1 | bash sandbox | `sandbox-exec` (`LAZAR_NO_SANDBOX`) |
| 2 | kernel binary lock | `chflags uchg bin/lazar` |
| 3 | source lock | `chmod a-w src/` |
| 4 | pre-tool vetoes | `hooks/pre-tool.d/*.sh` (renamed `.disabled`, not deleted) |
| 5 | agent instructions | `src/system-prompt.txt` + `access.rs` prompt context |

Layer 5 is the one people miss: with layers 1–4 off, the agent can *still*
self-censor if its prompt says a path is forbidden. The current mode is
therefore injected into every session by
`hooks/session-start.d/03-guardrails-mode.sh`, and the `guardrails` skill lets
the agent check and flip the switch itself.

State lives in `workspace/.guardrails`. An explicit `LAZAR_NO_SANDBOX` always
wins.

!!! note "Known tradeoff"
    Sub-agent state (`bin/lazar-spawn` budget counters) lives under
    `workspace/subagent/`, which is agent-writable — so an agent can reset its
    own `LAZAR_SPAWN_BUDGET` counter. This was a deliberate swap: the state
    previously lived in `logs/` (not agent-writable), which made spawn fail
    100% of the time under the sandbox. A working spawn with a soft budget
    beats a hard budget on a broken spawn.
