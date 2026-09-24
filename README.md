# lazar

The smallest self-evolving agent harness.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

One tool: `bash(command)` — bash, sandboxed.
One protocol: skills as filesystem entries.
Everything else is emergent.

## Architecture

```
~/lazar/
├── bin/lazar              kernel binary
├── bin/lazartui           native Rust ACP client
├── src/                   kernel crate
├── tui/                   Rust TUI crate (ratatui + crossterm)
├── scripts/               operator, build, and helper scripts
└── user/                  not in git: skills, memory, workspace, logs, hooks
```

The kernel is a small Rust runner, not an autopilot or self-modifier. It does three things:

1. Takes a prompt via `-p`.
2. Calls Claude (SSE-streamed) with one tool, `bash(command)`.
3. Runs the bash through `sandbox-exec` and feeds the output back.

That's it. State (memory, skills) lives on disk. Capabilities are markdown files
the agent reads and writes. Nested `lazar -p` recursion is intentionally API-key
isolated by default; set `LAZAR_TOOL_INHERIT_PROVIDER_KEY=1` on the parent Lazar
process only when tool subprocesses should be able to launch child API calls.
Only the active catalog provider's key is exposed. The older
`LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY` name remains a compatibility alias.

The kernel also speaks structured `--output-format stream-json` for
programmatic consumers (TUIs, log analyzers), records every event to
an append-only `logs/stream.jsonl`, and ships a few helper scripts
(`scripts/kernel-*.sh`, `scripts/lazar-chat.sh`). Everything beyond
that lives as skills. Kernel changes are intentionally outside the agent's runtime
write path and must be staged and applied through the proposal ceremony.

For infrastructure failover, `bin/lazar-failover <prompt-file> [provider ...]`
tries catalog providers in order. It retries only before a failed attempt emits
a hook or tool event; after either dispatch, replaying the prompt could duplicate
side effects. Each attempt starts with cleared session and model overrides.

The bundled `lazartui` keeps one `lazar acp` child alive for streaming chat,
sessions, cancellation, provider/model switching, scoped write access, markdown,
tool output, hook traces, and the raw kernel
event feed. Run `lazartui`, press F1 for the keymap, or use `/provider`, `/model`,
`/session`, and `/add-dir`. Provider secrets stay in environment variables or the
macOS Keychain; the TUI never reads or stores them.

## Kernel guardrails

The Rust kernel is the protected core. Runtime automation, hooks, skills, and
model-issued bash may read it, but sandboxed runtime commands must not write to
`src/` or `bin/`. There is no Rust-side autopilot/self-evolver that patches or
rebuilds the kernel on its own.

Kernel changes follow the proposal flow:

1. Run `scripts/new-proposal.sh <name>`, then edit the complete staged source,
   README, and fail-closed verification template under `workspace/proposals/`.
2. Run `scripts/kernel-apply.sh <name>`. It first moves the proposal into
   `.archive/kernel-proposals/`, outside the agent-writable sandbox, then prints
   the README, verification script, and exact frozen-source diff.
3. Confirm from an interactive operator terminal.
4. Let the script store a trusted backup under `.archive/kernel-backups/`,
   apply with checksum/delete semantics, rebuild with `Cargo.lock`, verify, and
   re-lock. Build or verification failure restores and rebuilds the backup.

The apply command prints a backup ID. A later regression can be reverted with
`scripts/kernel-rollback.sh <backup-id>`; rollback itself takes a safety backup
and requires interactive confirmation.

This keeps the core kernel operator-reviewed even though normal Lazar work can
continue in writable areas such as `skills/`, `memory/`, and `workspace/`.

### Toggling guardrails

Enforcement is a switch, not a wall. One command moves all five layers:

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
| 4 | pre-tool vetoes | `hooks/pre-tool.d/*.sh` (renamed `.guardrails-disabled`, not deleted) |
| 5 | **agent instructions** | `src/system-prompt.txt` + `access.rs` prompt context |

Layer 5 is the one people miss. With layers 1–4 off, the agent will *still*
refuse writes if its prompt says a path is forbidden — it self-censors while the
filesystem is wide open. The mode is therefore injected into every session by
`hooks/session-start.d/03-guardrails-mode.sh`, and the agent has a `guardrails`
skill so it can inspect the switch and give the operator the exact command.

State lives in the operator-owned `$LAZAR_HOME/.guardrails`, outside the
agent-writable sandbox. The CLI and ACP kernel read it when a process starts;
an explicit `LAZAR_NO_SANDBOX` or `--no-sandbox` also disables the sandbox.
Run the toggle from an operator terminal, then start a fresh invocation.

Guardrails OFF removes *enforcement*, not *discipline*: the proposal → review →
apply → verify → rollback flow above still works. `scripts/kernel-build.sh`
follows the persisted mode; `LAZAR_GUARDRAILS=on|off` overrides it for one
build without changing the persisted switch.

**Known tradeoff.** Sub-agent state (`bin/lazar-spawn`) lives under
`workspace/subagent/`, which is agent-writable. That includes the session spawn
counter enforcing `LAZAR_SPAWN_BUDGET` (default 10), so an agent *can* reset its
own budget. This was a deliberate swap: the state previously lived in `logs/`,
which no sandboxed agent can write, so spawn failed 100% of the time with
`Operation not permitted`. A working spawn with a soft budget beats a hard budget
on a broken spawn. If you need the budget tamper-proof, add a dedicated state
directory to the sandbox write allowlist rather than moving it back to `logs/`.

## Setup

Requires macOS (for `sandbox-exec` and `chflags uchg`), the Rust toolchain,
Python 3, `rsync`, `jq`, and `rg`.

### From a git clone (recommended)

```bash
git clone <repo-url> ~/lazar
cd ~/lazar
bash setup.sh

export ANTHROPIC_API_KEY=sk-...
lazar -p "what skills do you have?"
```

`bin/lazar` also works on its own: on first start it seeds anything missing under
`$LAZAR_HOME` (default `~/lazar`) from copies compiled into it — skills, hooks,
`user/providers.toml`, guardrails `off`, and the `bin/` helpers. It only ever
creates what is absent; existing skills, memory, sessions, logs and settings are
never overwritten.

Setup builds the kernel and TUI with external Cargo target directories
(`workspace/.cargo-target` and `workspace/.cargo-target-tui`), installs them as
`bin/lazar` and `bin/lazartui`, locks the kernel with `chflags uchg`, and seeds
`skills/` only when the skills directory has no `INDEX.md`. Re-running setup
preserves existing skills, memory, workspace, logs, and source unless you opt
into source update or reset.

To replace existing evolved skills and hooks with the current bundled minimal
set without deleting any other runtime state, run
`LAZAR_REFRESH_BUNDLES=1 bash setup.sh`. The old trees are retained read-only
under `.archive/runtime-bundles/`.

### What's in vs out of git

The repo ships the core plus the native client: `src/` (kernel source + seed
skills), `tui/`, `scripts/`, `setup.sh`, `README.md`, `.gitignore`, and the mkdocs
sources under `docs/docs/`. Everything generated at runtime or install time —
root `skills/`, `memory/`, most of `workspace/`, `logs/`, installed `bin/`
artifacts, Cargo targets, and `docs/site/` — is ignored by git. The former Go TUI
and its launch/watchdog artifacts are preserved locally under
`.archive/legacy-go-tui-2026-09-04/`; the only active TUI source is `tui/`.

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

This moves the directory to `~/lazar`, optionally removes a stale `/usr/local/bin/lazar` symlink (lazar is found through `PATH=~/lazar/bin:$PATH`), and shows diffs before patching any `clawd/lazar` references in your shell rc files (with backups). Use `--yes` for non-interactive migration. Skills, memory, workspace, and logs are preserved. You'll need to rebuild the kernel after the move — see the steps printed by the script.

If the script isn't present in your old install, copy it from the repo first:

```bash
cp ~/path-to-cloned-repo/scripts/migrate-from-clawd.sh ~/clawd/lazar/scripts/
bash ~/clawd/lazar/scripts/migrate-from-clawd.sh
```

## Use

```bash
# normal invocation
lazar -p "build a skill that prints the current time, then use it"

# reset everything but the kernel (factory restore; asks for confirmation)
lazar --reset-all

# non-interactive factory reset (destructive)
lazar --reset-all --yes

# get help
lazar --help
```

## Immutability

After `setup.sh` completes (guardrails ON — the default):

- `bin/lazar` has `chflags uchg` (OS-level immutable). Nothing — not the agent, not the user, not root — can modify or delete it without `chflags nouchg` first.
- `src/` is `chmod -R a-w`. The source is sealed.
- The sandbox profile (compiled into the binary via `include_str!`) blocks writes to `bin/` and `src/` from any bash command the agent runs.

The agent can `cat` the binary or the source — it can study itself. It cannot modify the runner. Evolution happens only in `skills/`.

This is the *default posture*, not a physical law. `scripts/lazar-guardrails.sh off`
lifts all of it (see [Toggling guardrails](#toggling-guardrails)). Run
`scripts/lazar-guardrails.sh status` if you are unsure which mode you are in.

## The infinite memory log

Every prompt, response, tool call, and tool result is appended as JSONL to:

    ~/lazar/logs/stream.jsonl

This is the agent's full history across every invocation, ever. The kernel does NOT auto-load it into the next conversation. Reading it is the agent's job, expressed as skills.

When you say `lazar -p "yes do that"`, the agent receives only that prompt — no prior turns. To recover context it must `tail` or `jq` the stream itself. The first time this fails noticeably, the right move is for the agent to author a context-loading skill so the failure never recurs.

That separation — kernel records, agent decides — is the architectural bet. Memory management, session scoping, summarization, recency heuristics: all of it lives in skills, where it can evolve.

## Reset

`lazar --reset-all` wipes `skills/`, `hooks/`, `memory/`, `workspace/`, `logs/`, and saved TUI access approvals, then re-seeds the embedded skills and hooks. It asks for confirmation unless you pass `--yes`. The kernel doesn't change. After reset, the agent is at the exact state of a fresh install.

`setup.sh` does not reset runtime state by default. If you deliberately want setup to perform a factory reset after rebuilding, run it as `LAZAR_RESET_ALL=1 bash setup.sh`.

The seed skills are:

- `_meta/create-skill` — how to author a new skill
- `_meta/find-capability` — how to decide whether a capability already exists
- `memory` — durable notes via plain markdown

## Upgrade

To rebuild the current installed kernel after editing `~/lazar/src`:

```bash
bash ~/lazar/scripts/kernel-unlock.sh
# edit src/, then rebuild + relock
bash ~/lazar/scripts/kernel-build.sh
```

If you are installing from a separate checkout and want to update an existing Lazar installation, setup preserves the installed `src/`, `tui/`, and `scripts/` trees unless you opt in. Use:

```bash
LAZAR_UPDATE_SOURCE=1 bash setup.sh
```

That creates a timestamped read-only backup under `.archive/source-updates/`
first. Runtime state is still preserved unless you also set
`LAZAR_RESET_ALL=1`.

If a source update includes fixes to bundled operational skills, opt into a
skill refresh separately:

```bash
LAZAR_UPDATE_SOURCE=1 LAZAR_REFRESH_BUNDLES=1 bash setup.sh
```

The previous skill and hook trees are archived under
`.archive/runtime-bundles/`; memory, workspace, logs, access approvals, and
provider configuration remain.

## Environment

Provider (catalog):

- `~/lazar/providers.toml` — the operator's provider catalog (id, endpoint, which env var / Keychain item holds the key, default model, model list). Seeded from `src/providers.toml` by setup; the kernel uses the built-in copy when the file is absent. `lazar providers` lists it, `lazar providers --json` feeds front ends. Neither prints secrets.
- `--provider <id>` / `LAZAR_PROVIDER` — select a catalog entry. It is authoritative for endpoint and key (stale `ANTHROPIC_*` residue is ignored); the key comes from the entry's env var, else from the macOS Keychain (`service=lazar`, `account=<env var>`, the `_meta/env-vars` convention) at depth 0 only. `--model` / `LAZAR_MODEL` still override the model. Propagates to tool subprocesses.

Provider (legacy env contract, unchanged when no provider is selected):

- `ANTHROPIC_API_KEY` — required by the top-level lazar process (sent as `x-api-key`). If unset, `MINIMAX_API_KEY` is used as a fallback together with the MiniMax Anthropic-compatible endpoint and model.
- `ANTHROPIC_BASE_URL` — override the API base URL (any Anthropic-compatible `/v1/messages` endpoint); default `https://api.anthropic.com`.
- `LAZAR_MODEL` — override the default model (`claude-sonnet-4-6`).

Paths and recursion:

- `LAZAR_HOME` — install/runtime root; defaults to `~/lazar`.
- `LAZAR_DEPTH` — recursion depth (set automatically for tool subprocesses; capped at 5).
- `LAZAR_TOOL_INHERIT_PROVIDER_KEY` — operator opt-in (`1`, `true`, `yes`, `on`) on the parent Lazar process. Exposes only the active provider's named key to tool subprocesses so a nested Lazar can authenticate. Off by default. `LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY` is a compatibility alias.

Agent-loop limits:

- `LAZAR_MAX_TURNS` — max model turns per invocation; default 50.
- `LAZAR_MAX_TOOL_CALLS` — max bash tool calls per invocation; default 200.
- `LAZAR_MAX_TOKENS` — `max_tokens` sent on every API call; default and compiled ceiling 8192.

Tools and hooks:

- `LAZAR_TOOL_TIMEOUT_SECS` — wall-clock timeout for each `bash(command)` tool call; default 120.
- `LAZAR_TOOL_OUTPUT_MAX_BYTES` — max stdout/stderr bytes kept from a tool call before truncation; default and compiled ceiling 8872.
- `LAZAR_HOOK_TIMEOUT_SECS` — per-hook wall-clock timeout; default 5.
- `LAZAR_STRICT_PRE_TOOL_HOOKS` — treat every pre-tool hook as fail-closed (a crash, timeout, or bad JSON becomes a veto).

Logs and sessions:

- `LAZAR_LOG_MAX_BYTES` — size at which `logs/stream.jsonl` auto-rotates; default 10485760.
- `LAZAR_LOG_ARCHIVE_KEEP` — rotated archives to keep once harvested; default 5, `0` disables pruning.
- `LAZAR_SESSION_HISTORY_MAX_BYTES` — cap on prior-turn history loaded by `--session` / `--resume`; default and compiled ceiling 32000. Oldest operator turns are dropped whole; tool calls and results are replayed as real blocks, never summarised.

Project access can be scoped per invocation:

```bash
lazar --cwd ~/Documents/GitHub/my-project \
  --add-dir ~/Documents/GitHub/my-project \
  -p "inspect this project"
```

- `--cwd <path>` sets the working directory for model-issued bash commands.
- Repeat `--add-dir <path>` to grant sandboxed writes to additional existing
  project roots. Paths are canonicalized before any hook or tool runs, inherited
  by nested Lazar calls, and rejected if they would overlap protected Lazar state.
- `--no-sandbox` remains the explicit full-access escape hatch.

## Sandbox boundaries

Every bash command runs through `sandbox-exec` with this policy:

- **Reads:** open. The agent can read its own kernel and source.
- **Writes:** only `skills/`, `memory/`, `workspace/`, `/tmp`, selected session/auth subdirectories, and any operator-approved `--add-dir` roots. `logs/` is trusted kernel-owned state: tools/skills may read it, not mutate it.
- **Network:** open (for API calls and skills like web-search).
- **Process spawn:** open (for CLI tools and optional `lazar -p` recursion; nested API recursion requires the parent to start with `LAZAR_TOOL_INHERIT_PROVIDER_KEY=1`).

Writes outside the allowed zones return `Operation not permitted`. The agent sees this in tool output and learns the boundary is real.

All of the above applies **only while guardrails are ON**. With
`scripts/lazar-guardrails.sh off` there is no `sandbox-exec` wrapper and no write
restrictions at all. Check with `scripts/lazar-guardrails.sh status`.

## Evaluating the harness

Model benchmarks answer *"how capable is the model?"*. `harness-evals` asks a
different question: **did this harness produce a complete, observable outcome?**

```bash
skills/harness-evals/bin/harness-evals selftest              # offline, free
skills/harness-evals/bin/harness-evals run --provider kimi   # live
skills/harness-evals/bin/harness-evals run --repeat 3        # consistency
skills/harness-evals/bin/harness-evals report                # trend
skills/harness-evals/bin/harness-evals compare <runA> <runB> # IMPROVE / REGRESS
skills/harness-evals/ci.sh --live --provider kimi            # CI gate
```

Required suites are `core` (real tool/result evidence, exact file state and
readback, tool-free chat) and `outcomes` (recovery after an observed tool
failure and state carried across separate edits). `spawn` is optional and
checks the installed delegation surface without launching a child.

Checks are **filesystem + event** based, never prose, so a model cannot talk its
way to a pass. Each run injects a random token to defeat memorisation.

It is backend-agnostic by design — same task packs, swappable adapter:

```bash
# any non-lazar agent that emits NDJSON
harness-evals run --cmd 'my-agent -p {{prompt}} > {{stream}}'
```

Two runs of the same model can differ, so use `--repeat` before trusting a
comparison. Timeouts, malformed JSONL, and missing terminal events are reported
as `error`, separately from behavioural `fail`; either blocks a required CI
gate because the run produced insufficient evidence. Results land in
`workspace/evals/results/<timestamp>/` with full transcripts. See
`skills/harness-evals/references/external-benchmarks.md` for how this layers
with Terminal-Bench / SWE-bench / Inspect AI.

## Design notes

- The runner never "knows about" specific capabilities. A new skill becomes available the moment its SKILL.md exists; no recompile, no registration code.
- Seed skills are baked into the binary so reset is fully self-sufficient — even if `src/` is gone, reset still works.
- The kernel appends every invocation to `logs/stream.jsonl` for replay and debugging, auto-rotates that log before it grows past `LAZAR_LOG_MAX_BYTES`, and owns archive pruning.
- Human output streams text live. `--output-format stream-json` emits structured JSONL events for TUIs and other programmatic consumers.
