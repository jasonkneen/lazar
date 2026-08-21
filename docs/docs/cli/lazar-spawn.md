# `lazar-spawn` CLI reference

`bin/lazar-spawn` is the **only sanctioned sub-agent spawn surface** — a locked
bash wrapper (installed by `setup.sh`, `chmod 555`) that sits between the agent
and raw `lazar -p` recursion. A pre-tool hook vetoes raw `lazar -p` calls from
the agent when the spawn-discipline hook is enabled.

Ground truth below comes from `bin/lazar-spawn` itself (it's readable bash) and
`skills/_meta/subagent/SKILL.md`.

## Usage

```text
lazar-spawn [--lint-only] [--detach] <task-id> <prompt-file>
lazar-spawn --status|--await <job-id>
```

| Flag | Effect |
|---|---|
| `--lint-only` | Validate the prompt file against the lint rules and exit — no child spawned |
| `--detach` | Spawn in the background; returns a job id immediately. Finalize later with `--status` / `--await` |
| `--status <job-id>` | Print the current job JSON envelope |
| `--await <job-id>` | Poll until the job finishes or `LAZAR_SPAWN_TIMEOUT` expires |

## Prompt file requirements

The child is **context-blind** — a stranger with a filesystem. The prompt file
must be self-contained:

- no more than 9000 bytes
- at least one absolute path
- a `SUCCESS:` line
- an `EXPECT-VERIFY:` block with indented claims:
  `exists=/abs`, `contains=/abs:needle`, `size_at_least=/abs:bytes`

The recommended pattern is a **context pack**: `pack-and-spawn.sh` writes a
`BRIEF.md` under `workspace/tasks/<id>/` (objective, absolute I/O paths,
SUCCESS/EXPECT-VERIFY claims, relevant skills, constraints, out-of-scope, and a
RESULT.md protocol), then dispatches a thin prompt pointing at the BRIEF.

```bash
bash "$LAZAR_HOME/skills/_meta/subagent/pack-and-spawn.sh" <task-id> \
  --objective "..." --out /absolute/deliverable
```

## Gate on claims

The wrapper **re-checks EXPECT-VERIFY claims against the filesystem** after the
child exits — the child's self-report is not trusted. The stdout envelope is
JSON:

```json
{"status":"ok","job_id":"...","task":"...","matched":true,"output":"/path/to/out.log"}
```

On failure you get `{"status":"fail","matched":false,"failed_claims":[...]}`
listing exactly which claims did not hold. Trust `matched`, not child prose. On
`matched:false`, fix the BRIEF and retry within the ceilings below.

A **sweeper** handles wedged jobs: if a detached worker's pid dies before
finalizing, `--status`/`--await` re-runs the stored claim check from the
child's out-log and finalizes the job itself.

## Bounds

1. Per-task ceiling `LAZAR_SPAWN_CEILING` (default 2)
2. Session budget `LAZAR_SPAWN_BUDGET` (default 10)
3. Wall-clock `LAZAR_SPAWN_TIMEOUT` (default 300s)
4. Kernel recursion depth cap (`MAX_DEPTH` = 5)
5. Prompt context cap (9000 bytes)

Budget/job state lives under `workspace/subagent/` — see the
[known tradeoff](../concepts/sandbox.md#the-guardrails-switch) about why it
isn't in `logs/`.

## Self-test

```bash
bash "$LAZAR_HOME/skills/_meta/subagent/selftest.sh"
```

See also: [Sub-agents integration guide](../integration/subagents.md).
