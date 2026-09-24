---
name: subagent
description: The ONLY sanctioned way to delegate work to a lazar sub-agent. Uses locked bin/lazar-spawn — lints the prompt, bounds retries, wall-clock-kills wedged children, and gates success by re-checking declared claims against the filesystem. Prefer pack-and-spawn.sh to write a BRIEF context pack then dispatch. Raw `lazar -p` is vetoed by a pre-tool hook. Use for isolated file-verifiable tasks and parallel fan-out — not for routine bash.
---

# subagent — how I extend myself

Delegation is a core skill. I never call `lazar -p` directly (a pre-tool hook
vetoes it). I go through the **locked** spawn surface, preferably with a
**context pack**:

```bash
# Recommended: BRIEF.md + thin prompt + lazar-spawn
bash "$LAZAR_HOME/user/skills/_meta/subagent/pack-and-spawn.sh" <task-id> \
  --objective "..." \
  --out /absolute/deliverable \
  --path /absolute/input \
  --skill create-skill \
  --note "user wants terse output"

# Lower-level (already-written prompt file):
bash "$LAZAR_HOME/bin/lazar-spawn" <task-id> <prompt-file>
```

`bin/lazar-spawn` is installed by `setup.sh` with the same immutability story
as `bin/lazar`. Budget state lives in `workspace/subagent/`. The copy under
`skills/_meta/subagent/spawn.sh` is **only** a re-exec stub.

## When to spawn vs do it yourself

| Do it yourself (bash in-process) | Spawn a child |
|----------------------------------|---------------|
| < ~3 tool steps | Isolated unit with a file deliverable |
| Needs live conversational deixis | Context isolation would help the parent |
| Exploratory one-off | 2+ independent units (parallel via agent-dispatch) |
| Result is just stdout to reason on | SUCCESS can be checked on disk |

**Never** wrap every bash call as a subagent — latency and token cost explode.

## Context pack (enough context without cloning chat)

Children are **strangers with a filesystem**. Do not dump parent history.
Write `workspace/tasks/<id>/BRIEF.md` (pack-and-spawn does this) containing:

1. Imperative objective
2. Absolute input/output paths
3. `SUCCESS:` + `EXPECT-VERIFY:` claims **you** author
4. Relevant skill names (not full INDEX)
5. ≤ ~20 lines of prior decisions / constraints
6. Out-of-scope list
7. RESULT.md protocol (`DONE` / `BLOCKED`)

The spawn `-p` string stays thin and points at the BRIEF.

## Gate on the envelope

`lazar-spawn` re-checks EXPECT-VERIFY with the kernel's checker
(`lazar verify`, the same claim language as a turn's `[VERIFY]` block) and
returns `{"status":"ok","matched":true}` only if claims hold. Trust `matched`, not
child prose. On `matched:false`, fix the BRIEF and retry within ceilings.

## Bounds (all models)

1. Prompt context cap (9000 bytes)
2. Per-task ceiling `LAZAR_SPAWN_CEILING` (default 2)
3. Session budget `LAZAR_SPAWN_BUDGET` (default 10)
4. Wall-clock `LAZAR_SPAWN_TIMEOUT` (default 300s)
5. Recursion depth cap (kernel `MAX_DEPTH`)

## Provider credentials

Children are credential-isolated by default. If they need model access, the
operator must start the parent Lazar process with
`LAZAR_TOOL_INHERIT_PROVIDER_KEY=1`; the kernel then passes only the active
catalog provider's named key to tool subprocesses. Setting the variable inside
a tool command is too late because that subprocess cannot change its parent.

## Self-test

`bash skills/_meta/subagent/selftest.sh`

## Async (optional)

```bash
bash "$LAZAR_HOME/bin/lazar-spawn" --detach <task-id> <prompt-file>
# → {"status":"running","job_id":"..."}

bash "$LAZAR_HOME/bin/lazar-spawn" --status <job-id>
bash "$LAZAR_HOME/bin/lazar-spawn" --await <job-id>
```

Prefer sync spawn for single tasks. Use `--detach` only for parallel fan-out; still gate on `matched` via --await/--status.
