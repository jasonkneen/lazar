---
name: subagent
description: The ONLY sanctioned way to delegate work to a lazar sub-agent. Uses locked bin/lazar-spawn (not agent-writable skills logic) — lints the prompt, bounds retries, wall-clock-kills wedged children, and gates success by re-checking declared claims against the filesystem. Raw `lazar -p` is vetoed by a pre-tool hook. Use whenever you need to fan out work, isolate context, or run something as a separate agent. Provider-agnostic.
---

# subagent — how I extend myself

Delegation is a core skill. I never call `lazar -p` directly (a pre-tool hook
vetoes it). I go through the **locked** spawn surface:

```bash
bash "$LAZAR_HOME/bin/lazar-spawn" <task-id> <prompt-file>
# equivalent thin re-exec (same binary):
bash skills/_meta/subagent/spawn.sh <task-id> <prompt-file>
```

`bin/lazar-spawn` is installed by `setup.sh` with the same immutability story
as `bin/lazar` (not under sandbox-writable `skills/`). Budget state lives in
`logs/subagent/` (sandbox cannot write logs/). The copy under `skills/` is
**only** a re-exec stub — never the real control plane.

This is the same path for every model/provider. No provider-specific shortcuts.

## The translation (run this on every delegation)

1. **Gate.** One task that needs my full context → I do it myself. Independent
   chunks or context-isolation helps → delegate.
2. **Decompose.** Units a stranger with zero context and only the filesystem
   can complete.
3. **Translate each unit into a self-contained spawn prompt**, written to a file:
   - Imperative objective.
   - **Absolute paths, never names** (`/Users/.../x.md`, not "the file").
   - A `SUCCESS:` line — the criterion in one sentence.
   - An `EXPECT-VERIFY:` block — claims that must become true, one per indented
     line: `exists=/abs`, `contains=/abs:token`, or `size_at_least=/abs:bytes`.
     I author these; the child cannot pass by declaring its own.
   - "Report honest failure; do not fabricate."
4. **Dispatch:** `bash "$LAZAR_HOME/bin/lazar-spawn" <task-id> <prompt-file>`.
   Never raw `lazar -p`.
5. **Gate on the envelope.** The wrapper re-checks EXPECT-VERIFY and returns
   `{"status":"ok","matched":true}` only if they hold. I trust `matched`, not
   the child's prose. On `matched:false` I fix the task and retry within bounds.
6. **Aggregate + terminate** on success-met or ceiling-hit. Never respawn the
   same failing prompt unbounded.

## Prompt example

```
Write the current UTC date to /Users/jkneen/lazar/workspace/out/stamp.txt.
SUCCESS: stamp.txt exists and contains a 2026 date
EXPECT-VERIFY:
  exists=/Users/jkneen/lazar/workspace/out/stamp.txt
  contains=/Users/jkneen/lazar/workspace/out/stamp.txt:2026
```

Dispatch: `bash "$LAZAR_HOME/bin/lazar-spawn" stamp /tmp/stamp-task.txt`

## Bounds (all models)

Three layers — do not remove one:

1. Kernel stall-nudges (in-child recovery, bounded)
2. Per-task ceiling `LAZAR_SPAWN_CEILING` (default 2)
3. Session budget `LAZAR_SPAWN_BUDGET` (default 10)

Wall-clock kill: `LAZAR_SPAWN_TIMEOUT` (default 300s).

## Self-test

`bash skills/_meta/subagent/selftest.sh` — lint, ceiling, claim re-check, veto,
and that production state is under `logs/subagent/`.
