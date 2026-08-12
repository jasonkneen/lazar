# Architecture

lazar is a **small immutable kernel** plus a **large writable everything-else**.
The split is the whole design.

```text
~/lazar/
├── bin/lazar              the immutable kernel (chflags uchg after build)
├── src/                   kernel source (read-only after build)
├── scripts/               kernel ceremony helpers + lazar-chat.sh wrapper
├── skills/                the agent's "being" — capabilities as folders
├── memory/                durable notes
├── workspace/             scratchpad, the agent's cwd, projects you build
└── logs/stream.jsonl      append-only event stream (kernel auto-rotates at ~10MB)
```

## The kernel

The kernel is a single Rust binary (`bin/lazar`, source in `src/`). It is a
runner, not an autopilot. It:

1. Parses CLI args (`clap`) — see the [CLI reference](../cli/lazar.md).
2. Assembles a system prompt from `src/system-prompt.txt` plus runtime
   identity (active model, API endpoint) and hook-injected context.
3. Runs an agent loop against an Anthropic-protocol `/v1/messages` endpoint,
   exposing exactly **one tool**: `bash(command)`.
4. Executes each tool call through `sandbox-exec` with the profile compiled in
   from `src/sandbox.sb` (unless `--no-sandbox` / `LAZAR_NO_SANDBOX=1`).
5. Appends every event (prompts, tool calls, results, hook fires) to
   `logs/stream.jsonl`, auto-rotating at `LAZAR_LOG_MAX_BYTES` (default 10 MB).
6. Fires lifecycle [hooks](../integration/hooks.md) at deterministic moments
   (session-start, user-prompt, pre-tool, post-tool, session-end,
   log-rotation, agent-stop, tick).
7. Enforces **VERIFY grounding**: when the agent ends a turn claiming file
   outcomes, the kernel parses its `[VERIFY]` block and re-checks every claim
   against the real filesystem. Failed claims trigger a bounded correction
   turn (`LAZAR_GROUNDING_RETRIES`, default 2), then a non-zero
   `grounding_failed` result.

Bounded limits keep runaways in check: `LAZAR_MAX_TURNS` (50),
`LAZAR_MAX_TOOL_CALLS` (200), `LAZAR_MAX_TOKENS` (16384),
`LAZAR_TOOL_TIMEOUT_SECS` (120), `LAZAR_STALL_NUDGES` (2), and a recursion
depth cap of 5 (`MAX_DEPTH`).

## The immutable kernel story

After `setup.sh`:

- `bin/lazar` has `chflags uchg` — OS-level immutable. Not even root can
  modify or delete it without `chflags nouchg` first.
- `src/` is `chmod -R a-w` — the source is sealed.
- The sandbox profile blocks writes to `bin/` and `src/` from any agent-run
  bash command.

The agent can `cat` its own binary and source — it can study itself — but it
cannot modify the runner. Kernel changes go through the
[proposal ceremony](kernel-guardrails.md), which requires operator review in an
interactive terminal.

## Seed skills and hooks

The seed skills (`src/seed-skills/`) and seed hooks (`src/seed-hooks/`) are
compiled into the binary with `include_dir!`. On first run — and after
`lazar --reset-all` — they are extracted to `skills/` and `hooks/`. This makes
`--reset-all` work fully offline: a factory-fresh agent needs nothing but the
binary.

## Where evolution happens

| Layer | Writable by agent? | Purpose |
|---|---|---|
| `bin/`, `src/` | **No** (locked + sandboxed) | The runner |
| `skills/` | Yes | Capabilities — markdown `SKILL.md` folders |
| `memory/` | Yes | Durable notes, journals, harness lessons |
| `workspace/` | Yes | Scratchpad, projects, task packs, proposals |
| `logs/` | Read-only (kernel-owned) | The canonical event stream |
| `hooks/` | Operator-controlled | Lifecycle scripts |

The kernel is **capability-blind**: dropping a new folder into `skills/` makes
the capability available immediately — no recompile, no registration. See
[Skills](../integration/skills.md).

## Sub-agents

Delegation goes through `bin/lazar-spawn`, a locked bash wrapper installed by
`setup.sh` that lints prompts, bounds retries, wall-clock-kills wedged
children, and re-checks the child's declared claims against the filesystem
before reporting success. See [Sub-agents](../integration/subagents.md).
