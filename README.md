# lazar

The smallest self-evolving agent harness.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

One tool: `execute(command)` — bash, sandboxed.
One protocol: skills as filesystem entries.
Everything else is emergent.

## Architecture

```
~/lazar/
├── bin/lazar              the immutable kernel (chflags uchg after build)
├── src/                   kernel source (read-only after build)
├── scripts/               kernel ceremony helpers + lazar-chat.sh wrapper
├── skills/                the agent's "being" — capabilities as folders
├── memory/                durable notes
├── workspace/             scratchpad, the agent's cwd, projects you build
└── logs/stream.jsonl      append-only event stream (rotated by skill at ~10MB)
```

The kernel is ~600 lines of Rust. It does three things:

1. Takes a prompt via `-p`.
2. Calls Claude (SSE-streamed) with one tool, `execute(command)`.
3. Runs the bash through `sandbox-exec` and feeds the output back.

That's it. Recursion (`lazar -p` calling itself) is just a tool call.
State (memory, skills) lives on disk. Capabilities are markdown files
the agent reads and writes.

The kernel also speaks structured `--output-format stream-json` for
programmatic consumers (TUIs, log analyzers), records every event to
an append-only `logs/stream.jsonl`, and ships a few helper scripts
(`scripts/kernel-*.sh`, `scripts/lazar-chat.sh`). Everything beyond
that lives as skills.

## Setup

Requires macOS (for `sandbox-exec` and `chflags uchg`) and the Rust toolchain.

### From a git clone (recommended)

```bash
git clone <repo-url> ~/lazar
cd ~/lazar
bash setup.sh

export ANTHROPIC_API_KEY=sk-...
lazar -p "what skills do you have?"
```

Setup builds in place (`~/lazar/src/`), installs the binary at `~/lazar/bin/lazar`, locks it with `chflags uchg`, and seeds `skills/` from the seed skills baked into the binary.

### What's in vs out of git

The repo ships only the core: `src/` (kernel source + seed skills), `scripts/` (kernel ceremony), `setup.sh`, `README.md`, `.gitignore`. Everything generated at runtime — `skills/` (user-evolved), `memory/`, `workspace/`, `logs/`, `bin/lazar`, `src/target/` — is ignored by git. Cloning gives you a clean install; running gives you your own personal evolution.

To publish your own fork:

```bash
cd ~/lazar
git init
git add .
git status   # confirm only source files are staged — runtime dirs should be ignored
git commit -m "init"
git remote add origin <your-repo>
git push -u origin main
```

## Skills are portable

Every skill under `src/seed-skills/` references paths via env vars (`$LAZAR_HOME`, `$LAZAR_SKILLS`, `$LAZAR_MEMORY`, `$LAZAR_WORKSPACE`, `$LAZAR_LOGS`) rather than hardcoded `~/lazar/`. The kernel exports these vars into every bash subprocess, so within lazar they Just Work.

This means **the skills themselves are portable across agents**. Any other agent harness that:

1. Can execute bash, and
2. Exports `LAZAR_HOME` (or rebinds the var to its own root)

…can drop the contents of `src/seed-skills/_meta/` into its own skills directory and inherit the patterns: bounded log reads, three-tier memory, shell-handoff via aliases, project context, kernel-patch staging. The patterns are the value; the implementation is just markdown + bash.

If you want to vendor the `_meta/` skills into a non-lazar agent: copy the folder, set `LAZAR_HOME` to your agent's root, and adjust `LAZAR_SKILLS`/`LAZAR_MEMORY`/etc. as needed. Or rename the env vars to your agent's convention with a single sed.

## Migrating from an older install

If you have a previous install at `~/clawd/lazar`, run:

```bash
bash ~/clawd/lazar/scripts/migrate-from-clawd.sh
```

This moves the directory to `~/lazar`, updates the `/usr/local/bin/lazar` symlink, and patches any `clawd/lazar` references in your shell rc files (with backups). Skills, memory, workspace, and logs are preserved. You'll need to rebuild the kernel after the move — see the steps printed by the script.

If the script isn't present in your old install, copy it from the repo first:

```bash
cp ~/path-to-cloned-repo/scripts/migrate-from-clawd.sh ~/clawd/lazar/scripts/
bash ~/clawd/lazar/scripts/migrate-from-clawd.sh
```

## Use

```bash
# normal invocation
lazar -p "build a skill that prints the current time, then use it"

# reset everything but the kernel (factory restore)
lazar --reset-all

# get help
lazar --help
```

## Immutability

After `setup.sh` completes:

- `bin/lazar` has `chflags uchg` (OS-level immutable). Nothing — not the agent, not the user, not root — can modify or delete it without `chflags nouchg` first.
- `src/` is `chmod -R a-w`. The source is sealed.
- The sandbox profile (compiled into the binary via `include_str!`) blocks writes to `bin/` and `src/` from any bash command the agent runs.

The agent can `cat` the binary or the source — it can study itself. It cannot modify the runner. Evolution happens only in `skills/`.

## The infinite memory log

Every prompt, response, tool call, and tool result is appended as JSONL to:

    ~/lazar/logs/stream.jsonl

This is the agent's full history across every invocation, ever. The kernel does NOT auto-load it into the next conversation. Reading it is the agent's job, expressed as skills.

When you say `lazar -p "yes do that"`, the agent receives only that prompt — no prior turns. To recover context it must `tail` or `jq` the stream itself. The first time this fails noticeably, the right move is for the agent to author a context-loading skill so the failure never recurs.

That separation — kernel records, agent decides — is the architectural bet. Memory management, session scoping, summarization, recency heuristics: all of it lives in skills, where it can evolve.

## Reset

`lazar --reset-all` wipes `skills/`, `memory/`, `workspace/`, `logs/` and re-seeds the skills directory from copies embedded in the binary at compile time. The kernel doesn't change. After reset, the agent is at the exact state of a fresh install.

The seed skills are:

- `_meta/create-skill` — how to author a new skill
- `_meta/find-capability` — how to decide whether a capability already exists
- `memory` — durable notes via plain markdown

## Upgrade

To rebuild the kernel:

```bash
chflags nouchg ~/lazar/bin/lazar
chmod +w ~/lazar/bin/lazar
chmod -R u+w ~/lazar/src
# edit src/, then re-run
bash setup.sh
```

## Environment

- `ANTHROPIC_API_KEY` — required.
- `LAZAR_MODEL` — override the default model (`claude-sonnet-4-6`).
- `LAZAR_DEPTH` — recursion depth (set automatically when the agent calls itself; capped at 5).

## Sandbox boundaries

Every bash command runs through `sandbox-exec` with this policy:

- **Reads:** open. The agent can read its own kernel and source.
- **Writes:** only `skills/`, `memory/`, `workspace/`, `logs/`, and `/tmp`.
- **Network:** open (for API calls and skills like web-search).
- **Process spawn:** open (for `lazar -p` recursion and any tool the agent invokes).

Writes outside the allowed zones return `Operation not permitted`. The agent sees this in tool output and learns the boundary is real.

## Design notes

- The runner never "knows about" specific capabilities. A new skill becomes available the moment its SKILL.md exists; no recompile, no registration code.
- Seed skills are baked into the binary so reset is fully self-sufficient — even if `src/` is gone, reset still works.
- The kernel logs every invocation to `logs/<unix-millis>.json` for replay and debugging.
- There is no streaming. Add it as a skill (it can wrap `lazar -p` and stream the output) rather than baking it into the kernel.
