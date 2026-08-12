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

## What is lazar?

lazar is a small Rust runner that gives a language model exactly one tool —
sandboxed `bash` — and gets out of the way. There is no autopilot, no planner
framework, no plugin SDK. The agent's *capabilities* are markdown files
("skills") on disk that it reads and writes itself. State lives on disk.
Everything else is emergent.

The kernel does three things:

1. Takes a prompt via `-p`.
2. Calls an Anthropic-protocol model endpoint (SSE-streamed) with one tool, `bash(command)`.
3. Runs the bash through `sandbox-exec` and feeds the output back.

## Quick links

- [Install](getting-started/install.md) — clone, `setup.sh`, first prompt
- [Configuration](getting-started/configuration.md) — API keys, model selection, backends
- [Architecture](concepts/architecture.md) — kernel vs skills vs memory vs workspace
- [`lazar` CLI reference](cli/lazar.md) — every flag, verified from `--help`
- [Environment variables](api/env-vars.md) — the complete `LAZAR_*` table
- [Hooks protocol](integration/hooks.md) — lifecycle scripts in `hooks/<event>.d/`
- [Sub-agents](integration/subagents.md) — the `bin/lazar-spawn` delegation surface

## The bet

The kernel records *everything* to an append-only JSONL stream and deliberately
does **not** auto-load it into the next conversation. Memory management, context
loading, summarization, recency heuristics — all of it lives in skills, where it
can evolve. The kernel is sealed (`chflags uchg`); the agent can read its own
source but can never modify its runner. Evolution happens only in `skills/`.
