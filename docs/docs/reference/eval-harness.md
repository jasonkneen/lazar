# Eval harness

Lazar ships an outcome-based evaluation suite that scores **the harness**, not
assistant claims: structured tool evidence, exact filesystem state, recovery
after a failed tool result, and terminal completeness.

## Where it lives

- **Skill:** `skills/harness-evals/` (vendored snapshot so `--reset-all`
  works offline).
- **Seed source:** `src/seed-skills/harness-evals/`.
- **Run output:** ignored `workspace/evals/results/`.

## When to run it

- Comparing providers/models on the same harness.
- Proving a harness change helped or regressed.
- CI gates before shipping kernel/skill changes.
- Detecting stalled, malformed, narrated-only, or state-incomplete runs.

## How it works

The same task packs run against Lazar or a foreign JSONL-producing command.
The scorer strictly parses every stream line and checks structured events plus
the isolated task filesystem. It never searches assistant prose for success,
failure, or honesty words. Required timeouts and malformed/incomplete streams
are CI failures because there is not enough evidence to judge the outcome.
A nonzero adapter exit code overrides apparently successful stream checks.
Completion is judged on the final invocation; an earlier completed invocation
cannot finish a later one. Raw `message_stop` requires an actual successful
stop reason and cannot complete a pending tool round.

```bash
skills/harness-evals/bin/harness-evals selftest
skills/harness-evals/bin/harness-evals run --suite core,outcomes
skills/harness-evals/bin/harness-evals run --provider kimi --repeat 3
ls workspace/evals/results/                 # prior live runs
```

See `skills/harness-evals/SKILL.md` for adapter configuration and pack
authoring.
