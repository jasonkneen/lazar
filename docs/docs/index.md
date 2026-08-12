# lazar

**The smallest self-evolving agent harness.**

One tool: `bash(command)` — bash, sandboxed.
One protocol: skills as filesystem entries.
Everything else is emergent.

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

## Tent-pole features

**1. The agent can't lie about what it did — [VERIFY contract](concepts/verify-contract.md)**
When the agent claims it created, rotated, or fixed a file, the kernel parses
its `[VERIFY]` block and re-checks every claim against the real filesystem
(`stat`, `exists`, `contains`) before accepting the answer. Failed claims force
a correction turn; a second failure ends the run with `grounding_failed`.
"Done" is verified, not trusted — and the check is compiled into the kernel,
so the agent can never patch it away.

**2. Sealed kernel, evolving agent — [Architecture](concepts/architecture.md)**
The runner is locked with `chflags uchg` after build. The agent can read its
own source but can never modify it. All growth happens in `skills/` — markdown
files the agent reads and writes itself. New capability needed? The agent
authors a new skill. The kernel stays small and permanent; the *being* grows.

**3. Every event, on disk, forever — [Sessions & memory](integration/sessions.md)**
The kernel appends everything — prompts, responses, tool calls, results,
verify reports — to an append-only JSONL stream, and deliberately does **not**
auto-load it into the next conversation. Memory management, context loading,
summarization, recency heuristics: all of it lives in skills, where it can
evolve. Nothing is hidden in a black-box database.

**4. One tool, full power, real guardrails**
The agent gets exactly one tool: `bash(command)`, run through `sandbox-exec`.
Write zones, blocked paths, and command vetoes are enforced at the syscall
boundary — not by asking the model nicely. Guardrails can be toggled off
explicitly (`lazar-guardrails.sh off`), and the system prompt tells the agent
which mode it's in so it never guesses.

**5. Delegation with receipts — [Sub-agents](integration/subagents.md)**
`bin/lazar-spawn` runs child agents for isolated, file-verifiable work. Child
prompts must be stranger-complete (absolute paths, a SUCCESS line, EXPECT-VERIFY
claims authored by the parent), and the spawn binary re-checks the child's
declared claims before reporting success. Bounded retries, wedged-child kills,
recursion depth caps — no runaway agent trees.

**6. A lifecycle you can hook — [Hooks protocol](integration/hooks.md)**
User-controlled scripts in `hooks/<event>.d/` fire at session-start,
user-prompt, pre-tool, post-tool, session-end, and tick. Hooks can inject
context, veto tool calls, and shape behavior — without touching the kernel.

## What is lazar?

lazar is a small Rust runner that gives a language model exactly one tool —
sandboxed `bash` — and gets out of the way. There is no autopilot, no planner
framework, no plugin SDK. State lives on disk. Everything else is emergent.

The kernel does three things:

1. Takes a prompt via `-p`.
2. Calls an Anthropic-protocol model endpoint (SSE-streamed) with one tool, `bash(command)`.
3. Runs the bash through `sandbox-exec` and feeds the output back.

## Quick links

- [Install](getting-started/install.md) — clone, `setup.sh`, first prompt
- [Configuration](getting-started/configuration.md) — API keys, model selection, backends
- [Architecture](concepts/architecture.md) — kernel vs skills vs memory vs workspace
- [VERIFY contract](concepts/verify-contract.md) — kernel-enforced honesty: the agent can't lie about file state
- [`lazar` CLI reference](cli/lazar.md) — every flag, verified from `--help`
- [Environment variables](api/env-vars.md) — the complete `LAZAR_*` table
- [Hooks protocol](integration/hooks.md) — lifecycle scripts in `hooks/<event>.d/`
- [Sub-agents](integration/subagents.md) — the `bin/lazar-spawn` delegation surface

## The bet

A language model plus one honest tool, a sealed runner, and a filesystem it
can grow into is enough. Capabilities are data, not code paths. Trust is
enforced, not assumed. The interesting parts — memory, judgment, delegation,
self-repair — are all skills, which means they improve every session instead
of waiting for a kernel release.
