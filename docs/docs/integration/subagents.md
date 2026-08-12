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
returns `{"status":"ok","matched":true}` only if they hold. **Trust
`matched`, not child prose.**

## Bounds

| Bound | Default |
|---|---|
| Kernel stall-nudges (in-child) | bounded, `LAZAR_STALL_NUDGES` |
| Per-task retries (`LAZAR_SPAWN_CEILING`) | 2 |
| Session spawn budget (`LAZAR_SPAWN_BUDGET`) | 10 |
| Wall-clock kill (`LAZAR_SPAWN_TIMEOUT`) | 300s |
| Recursion depth (`LAZAR_DEPTH` vs kernel `MAX_DEPTH`) | 5 |

## The auth gotcha

Children do **not** inherit the provider API key by default. A child spawned
without credentials fails its first API call with `401 invalid x-api-key` and
the spawn returns `status: fail` with every claim unmet. Pass
`LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY=1` on the spawn when the child needs
model access (which is almost always).

## When NOT to spawn

Routine bash, exploratory one-offs, anything whose result is stdout you need
to reason on — do those in-process. Spawn only for isolated, file-verifiable
units of work or parallel fan-out.
