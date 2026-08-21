# Sub-agents

Delegation goes through one locked binary: `bin/lazar-spawn`. Raw
`lazar -p` from inside an agent session is vetoed by a pre-tool hook.

## Spawning

```bash
# Recommended: context pack + dispatch
bash "$LAZAR_HOME/skills/_meta/subagent/pack-and-spawn.sh" <task-id> \
  --objective "..." --out /absolute/deliverable --path /absolute/input

# Lower-level: you already wrote the prompt file
bash "$LAZAR_HOME/bin/lazar-spawn" <task-id> <prompt-file>
```

## The context pack (BRIEF.md)

Children are strangers with a filesystem — no parent history. Write
`workspace/tasks/<id>/BRIEF.md` containing:

1. Imperative objective
2. Absolute input/output paths
3. `SUCCESS:` + `EXPECT-VERIFY:` claims authored by the parent
4. Relevant skill names
5. Prior decisions / constraints (≤ ~20 lines)
6. Out-of-scope list
7. `RESULT.md` protocol (first line `DONE` or `BLOCKED`)

`lazar-spawn` re-checks the EXPECT-VERIFY claims against the filesystem and
returns `{"status":"ok","matched":true}` only if at least one claim was
declared, all claims hold, and the child exits successfully. **Trust
`matched`, not child prose.**

## Bounds

| Bound | Default |
|---|---|
| Per-task retries (`LAZAR_SPAWN_CEILING`) | 2 |
| Session spawn budget (`LAZAR_SPAWN_BUDGET`) | 10 |
| Wall-clock kill (`LAZAR_SPAWN_TIMEOUT`) | 300s |
| Recursion depth (`LAZAR_DEPTH` vs kernel `MAX_DEPTH`) | 5 |

## The auth gotcha

Children do **not** inherit the provider API key by default. A child spawned
without credentials fails its first API call and the spawn returns
`status: fail` with every claim unmet. When children need model access, the
operator must start the parent invocation with the explicit opt-in:

```bash
LAZAR_TOOL_INHERIT_PROVIDER_KEY=1 lazar --provider minimax -p "..."
```

Only the active provider's named key is passed to tool subprocesses. Setting
the variable inside a model-issued command is too late because subprocesses
cannot mutate the parent kernel's environment.

## When NOT to spawn

Routine bash, exploratory one-offs, anything whose result is stdout you need
to reason on — do those in-process. Spawn only for isolated, file-verifiable
units of work or parallel fan-out.

## Detached jobs

`lazar-spawn --detach <task-id> <prompt-file>` starts an independently owned
supervisor and returns its job ID and PID. Its state is under
`workspace/subagent/jobs/`; `--status <job-id>` and `--await <job-id>` read that
record. Detached work intentionally survives the originating tool call.
The supervisor bounds runtime, cleans the child process group, and writes the
final result atomically. A lost supervisor is an error, even if output files
happen to match. Empty claim blocks are rejected before any child launches.

Both foreground and detached execution require the installed Python helpers
`bin/run-with-timeout.py` and `bin/spawn-supervisor.py`. The timeout runner has
a separate watchdog that cleans its child group if the runner is killed.
Session budget updates use a short POSIX file lock, including on macOS.
