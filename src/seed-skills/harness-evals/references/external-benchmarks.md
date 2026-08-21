# External agent benchmarks (and how they relate)

`harness-evals` measures the **harness**. These measure the **model** (or the
model+harness together). They are complementary, not competing.

## Layering

| Layer | Question | Tool |
|---|---|---|
| L0 | Does the kernel behave? | kernel integration tests, `_meta/health-check` |
| L1 | Does the harness produce observable, complete outcomes? | **harness-evals (this skill)** |
| L2 | How capable is the agent on real work? | external benches below |

## Coding / terminal agents
- **Terminal-Bench** (https://www.tbench.ai/) — shell tasks; closest fit for a bash-first agent
- **SWE-bench / Verified** (https://www.swebench.com/) — real GitHub issue → patch
- **SWE-agent / mini-SWE-agent** (https://github.com/SWE-agent/SWE-agent) — reference harness + runner
- **Aider leaderboards** (https://aider.chat/docs/leaderboards/) — edit-quality comparison

## Tool-use / multi-turn
- **tau-bench** (https://github.com/sierra-research/tau-bench) — policy-constrained tool use
- **BFCL** (https://gorilla.cs.berkeley.edu/leaderboard.html) — function-calling correctness
- **AgentBench**, **WebArena**, **GAIA**

## Generic eval runners (good hosts for L1 + L2)
- **Inspect AI** (https://inspect.aisi.org.uk/) — solvers + scorers; best fit to wrap lazar
- **DeepEval** (https://github.com/confident-ai/deepeval) — pytest-style metrics
- **Promptfoo** (https://www.promptfoo.dev/) — YAML scenarios, CI-friendly

## Why not just use those?
They score broader task success. They do not necessarily isolate:
- whether a command appeared in a real tool event and result
- whether exact filesystem state matches the requested outcome
- whether the loop continued after an observed tool failure
- whether the structured stream terminated cleanly

Those are harness properties, and they are what this suite exists to protect.

## Suggested integration order
1. Keep `harness-evals` green (fast, cheap, every change).
2. Wrap lazar as an **Inspect AI** solver.
3. Add **Terminal-Bench** for external capability comparison.
