# Eval harness

lazar ships a backend-agnostic evaluation suite that scores **the harness**,
not the model: tool honesty, VERIFY grounding, fabrication resistance,
recovery, self-learning surfaces. Two agents with the same model can score
very differently — that delta is harness quality.

## Where it lives

- **Skill:** `skills/harness-evals/` (vendored snapshot so `--reset-all`
  works offline).
- **Upstream:** `/Users/jkneen/Documents/GitHub/harness-evals` — canonical,
  agent-agnostic; make changes there.
- **Workspace runner:** `workspace/evals/` with `run-all.sh`,
  `kernel-evals.sh`, `model-evals.sh`, `tests/`, `results/`.

## When to run it

- Comparing backends/models on the same harness.
- Proving a harness change helped or regressed.
- CI gates before shipping kernel/skill changes.
- Answering "is the self-learning system actually working?"

## How it works

The same task packs run against any agent via pluggable adapters (lazar,
generic command, recorded fixtures). Scores target harness behaviors:
whether the agent fabricates file state, whether its `[VERIFY]` claims match
the filesystem, whether it recovers from induced failures, and whether
lessons get recorded durably.

```bash
bash ~/lazar/workspace/evals/run-all.sh     # full suite
ls   ~/lazar/workspace/evals/results/       # prior runs
```

See `skills/harness-evals/SKILL.md` for adapter configuration and pack
authoring.
