---
name: subagent
description: The ONLY sanctioned way to delegate work to a lazar sub-agent. Translates a requirement into a self-contained, verifiable spawn through spawn.sh — which lints the prompt, bounds retries, wall-clock-kills wedged children, and gates success by re-checking your declared claims against the filesystem. Raw `lazar -p` is vetoed by a pre-tool hook. Use whenever you need to fan out work, isolate context, or run something as a separate agent.
---

# subagent — how I extend myself

Delegation is a core skill. I never call `lazar -p` directly (a pre-tool hook vetoes it); I go through `spawn.sh`. Every external orchestration pattern — imported packs, a user ask, a self-generated goal — is a *source of intent* I decompose and run through THIS process, never executed raw.

## The translation (run this on every delegation)

1. **Gate.** Is this a sub-agent job? One task that needs my full context → I do it myself. Independent chunks, or context-isolation helps → delegate.
2. **Decompose.** Break the requirement into units that survive being read by a stranger with zero context and only filesystem access.
3. **Translate each unit into a self-contained spawn prompt**, written to a file:
   - Imperative objective.
   - **Absolute paths, never names** (`/Users/.../x.md`, not "the file").
   - A `SUCCESS:` line — the criterion in one sentence.
   - An `EXPECT-VERIFY:` block — the claims that must become true, one per indented line: `exists=/abs`, `contains=/abs:token`, or `size_at_least=/abs:bytes`. I author these; the child cannot pass by declaring its own.
   - "Report honest failure; do not fabricate."
4. **Dispatch:** `bash skills/_meta/subagent/spawn.sh <task-id> <prompt-file>`. Never raw `lazar -p`.
5. **Gate on the envelope.** spawn.sh re-checks my EXPECT-VERIFY claims itself and returns `{"status":"ok","matched":true}` only if they hold. I trust `matched`, not the child's prose. On `matched:false` I read `failed_claims`, fix the task, and retry — the wrapper bounds me (per-task ceiling, session budget) so a broken task can't loop.
6. **Aggregate + terminate** on success-met or ceiling-hit. I never respawn the same failing prompt.

## Prompt example

```
Write the current UTC date to /Users/jkneen/lazar/workspace/out/stamp.txt.
SUCCESS: stamp.txt exists and contains a 2026 date
EXPECT-VERIFY:
  exists=/Users/jkneen/lazar/workspace/out/stamp.txt
  contains=/Users/jkneen/lazar/workspace/out/stamp.txt:2026
```

Dispatch: `bash skills/_meta/subagent/spawn.sh stamp /tmp/stamp-task.txt`

## Honest limits

With weak child models this DETECTS flakiness and retries within the ceiling; it does not prevent it. Three named layers bound the blast radius — do not remove one: kernel stall-nudges (in-child recovery) → per-task/prompt ceiling `LAZAR_SPAWN_CEILING` (default 2, retry bound) → session budget `LAZAR_SPAWN_BUDGET` (default 10, absolute stop). The wall-clock kill `LAZAR_SPAWN_TIMEOUT` (default 300s) stops wedged children.

## Self-test

`bash skills/_meta/subagent/selftest.sh` re-verifies the whole discipline (lint, ceiling, claim re-check, veto). Run it after any change to this skill — an executable check outlives a prose promise.
