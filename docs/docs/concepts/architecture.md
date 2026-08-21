# Architecture

lazar is a **small immutable kernel** plus a **large writable everything-else**.
The split is the whole design.

```text
~/lazar/
├── bin/lazar              kernel binary
├── bin/lazartui           native Rust ACP client, built from tui/
├── src/                   kernel crate (sources in src/src)
├── tui/                   Rust TUI crate (ratatui). The only active client source
├── scripts/               operator, build, migration, and chat helpers
└── user/                  not in git — skills, memory, workspace, logs, hooks
```

## The kernel

The kernel is a single Rust binary (`bin/lazar`, crate root `src/`). It is a
runner, not an autopilot. The bundled client is a second crate, `tui/`,
installed as `bin/lazartui`. It speaks ACP to one long-lived `lazar acp`
process. The archived Go client is not part of the build. The kernel:

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
7. Trusts the provider's `stop_reason`: `tool_use` executes and continues,
   a terminal reason ends the turn. The kernel never inspects the model's
   prose to decide whether the turn is "really" finished and never re-prompts
   on the operator's behalf. Recovering a model that narrates instead of
   calling the tool is the operator's (or a better model's) job.

Bounded limits keep runaways in check: `LAZAR_MAX_TURNS` (50),
`LAZAR_MAX_TOOL_CALLS` (200), `LAZAR_MAX_TOKENS` (8192),
`LAZAR_TOOL_TIMEOUT_SECS` (120), and a recursion
depth cap of 5 (`MAX_DEPTH`).

## The immutable kernel story

After `setup.sh`:

- The sandbox is off unless `--sandbox`, `LAZAR_SANDBOX=1`, or `user/.guardrails`
  says `on`.
- `src/` and `tui/` stay writable. Self-learning belongs in `user/memory` and
  `user/skills`.

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

## Turn lifecycle and context limits

The Rust TUI in `tui/` owns presentation and a long-lived ACP connection. ACP
runs prompts as concurrent connection tasks so cancel notifications remain
processable during model requests, tools, and hooks. On TUI shutdown or
restart, stdin EOF asks ACP to cancel and drain workers before running
session-end hooks. Kernel-launched command groups also have a guardian tied
to an owner pipe. If the kernel is killed abruptly, the guardian terminates
its command group; this composes through nested Lazar invocations. The pipe
is close-on-exec so a command cannot accidentally keep its owner alive.
Stderr diagnostics do not finish a turn; the prompt response
or a disconnected transport does. An access change is rejected during a turn.

Operator and skill-expanded prompts use `ContextualUserFragment` wrappers.
This type boundary covers those two inputs only. System and injected hook
context use explicit string validation; tool results and replay use bounded
transformations. The per-item ceiling is 9,000 bytes, injected hook context is
4,000 bytes, replay is at most 32,000 serialized bytes, and each complete next
request is capped at 64,000 serialized bytes. These are conservative byte
budgets, not model-specific token counts or a universal 1,000-token policy.

Replay retains available tool exit codes separately from displayed output.
Provider requests translate nonzero status to `is_error` and omit Lazar's
`exit_code` field. Replay may truncate text, omit oversized historical tool
inputs together with their results, repair interrupted tool/result pairs, and
drop old operator turns. It is not byte-for-byte or prefix-stable replay. If no
valid continuation survives for an existing conversation, a prompt fails with
an explicit context-limit message instead of silently starting fresh.

A terminal protocol response says that an invocation stopped. It does not
prove the requested work was done. Outcome checks belong in callers such as
`lazar-spawn` and the eval harness, using declared checks and observed results.
Consumers tracking pending actions must clear them from corresponding result
evidence; assistant wording is not a completion signal.
