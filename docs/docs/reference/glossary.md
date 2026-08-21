# Glossary

**agent loop** — the kernel's cycle of model call → tool call → tool result →
model call. Lives in the kernel only, never in the TUI or any client.

**BRIEF.md** — the context pack handed to a sub-agent: objective, absolute
paths, SUCCESS + EXPECT-VERIFY claims, constraints, RESULT.md protocol. See
[Sub-agents](../integration/subagents.md).

**continual harness** — the ranked, durable lesson store in
`memory/harness/`, managed by `skills/_meta/harness/harness.py`. Overview is
auto-injected at session start.

**event stream** — `logs/stream.jsonl`, the append-only JSONL record of every
prompt, response, tool call, and result. Recorded by the kernel, loaded only
via bounded recipes.

**EXPECT-VERIFY** — the testable claims a parent authors for a spawned task;
`lazar-spawn` re-checks them against the filesystem before reporting
`matched: true`.

**grounding** — the discipline of proving file-state claims with real
commands (run the tool, read the output) rather than narrating an outcome.

**guardrails** — the on/off switch for sandbox + trusted-zone protections,
toggled by `scripts/lazar-guardrails.sh` (the `guardrails` skill).

**heartbeat / tick** — the `lazar --tick` path: a scheduled invocation that
walks maintenance phases (journal, todos, memory hygiene) with zero user
questions.

**hooks** — user-controlled bash scripts in `hooks/<event>.d/` fired by the
kernel at lifecycle moments. See [Hooks](../integration/hooks.md).

**kernel** — the immutable `bin/lazar` binary + `src/` source. Read-only
after build; evolution happens via skills, never kernel edits.

**lazar-spawn** — the locked delegation binary. The only sanctioned way to
run a sub-agent; raw `lazar -p` from inside a session is vetoed.

**memory** — plain-markdown persistence under `memory/`. See
[Memory](../integration/memory.md).

**pack-and-spawn** — `skills/_meta/subagent/pack-and-spawn.sh`: writes a
BRIEF.md context pack then dispatches via `bin/lazar-spawn`.

**PINNED.md** — `memory/PINNED.md`, the session-start ground truth: provider
config, code ownership, behavioral rules.

**RESULT.md** — the sub-agent status file; first line `DONE` or `BLOCKED`.

**sandbox** — the `sandbox-exec` profile limiting writes to `skills/`,
`memory/`, `workspace/`, `/tmp`, and specific session subdirs when
guardrails are on. See Concepts → Sandbox.

**session** — opt-in continuity via `--session <id>` / `--resume`. See
[Sessions](../integration/sessions.md).

**skill** — a folder under `skills/` with a `SKILL.md`; the only extension
mechanism. See [Skills](../integration/skills.md).

**TUI** — the native Rust terminal client in `tui/`, installed as
`bin/lazartui`. It drives one long-lived `lazar acp` child and contains no agent
loop. The former Go client is archived under the local ignored `.archive/`
directory, not under the active runtime workspace.


**work journal** — `memory/work-journal.md`, the durable record of completed
work, harvested from the event stream.
