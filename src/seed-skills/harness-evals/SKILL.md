---
name: harness-evals
description: Outcome-based evaluation for Lazar and other coding-agent harnesses. Scores strict structured events, exact filesystem state, tool failure recovery, and terminal completeness. Never treats assistant prose as evidence.
---

# harness-evals

Use this skill to test the agent loop and harness, not to rank model knowledge.
Every required pass is based on structured stream events or filesystem state.
There is no prose classifier and no dependency on removed `verify_report` or
`grounding_*` events.

## Quick start

```bash
# Offline scorer, fixture, task-pack, and portable-timeout tests
bash "$LAZAR_HOME/user/skills/harness-evals/bin/harness-evals" selftest

# Live default suites through the current provider
bash "$LAZAR_HOME/user/skills/harness-evals/bin/harness-evals" run

# Use an explicit provider ID from the live catalog
"$LAZAR_HOME/bin/lazar" providers --json
bash "$LAZAR_HOME/user/skills/harness-evals/bin/harness-evals" run --provider kimi

# Repeat before comparing a noisy model/provider result
bash "$LAZAR_HOME/user/skills/harness-evals/bin/harness-evals" run --repeat 3
```

`ci.sh` runs offline tests by default. `ci.sh --live` also runs the required
live suites. A required timeout, malformed stream, missing terminal event, or
behavioral failure makes the live gate nonzero.

## Required suites

| Suite | What it proves |
|---|---|
| `core` | real tool/result evidence, exact file state plus readback, clean tool-free chat |
| `outcomes` | recovery after a failed tool result and state carried across separate edits |

`spawn` is optional and checks that the locked delegation surface is installed;
it never launches a child.

## Evidence model

Supported checks:

- `stream_tool_ran` and `stream_tool_result_contains`
- `stream_min_tool_calls` and `stream_max_tool_calls`
- `stream_failed_tool_result` and `stream_successful_tool_result`
- `stream_end_ok` and `stream_has_event`
- `text_nonempty` and `text_contains`
- `fs_exists`, `fs_not_exists`, `fs_contains`, and `fs_exact`
- `any_fs_exists`

The scorer parses every nonblank stream line as a JSON object. Invalid JSON,
an empty stream, a started invocation without a terminal event, or an `error`
event is infrastructure status `error`; it cannot partially pass. A complete
run with unmet evidence is status `fail`. Optional tasks become
`skip_optional` and do not gate CI.

Assistant text can satisfy only explicit text checks. Claims such as "I wrote
the file" or "the command succeeded" never satisfy a filesystem or tool check.

## Adapters

### Lazar

The default adapter invokes:

```text
bin/lazar --output-format stream-json --cwd <isolated-workdir> --add-dir <isolated-workdir>
```

It does not disable the sandbox. `--provider` is passed directly to the kernel,
whose `providers.toml` catalog and available credential are authoritative.

### Foreign command

```bash
harness-evals run --adapter command \
  --cmd 'my-agent --jsonl -p {{prompt}} > {{stream}}'
```

Placeholders are expanded as shell-escaped argv tokens. Do not wrap them in
additional quotes. Available placeholders are `{{prompt}}`, `{{workdir}}`,
`{{stream}}`, `{{token}}`, and `{{lazar_home}}`.

The foreign process must emit one JSON object per line and finish with a
terminal event such as:

```json
{"type":"invoke_end","stop_reason":"end_turn"}
```

The portable Python timeout runner terminates the entire adapter process group
at the task deadline. The adapter exit code, stderr, stream, expanded prompt,
task, context, and score are all retained in the result directory.

### Recorded stream

Use the scorer directly for one recorded transcript:

```bash
harness-evals score --stream run.jsonl --task task.json --ctx '{"token":"ABC"}'
```

## Results and comparison

Runs land under `workspace/evals/results/<utc>.<pid>/`:

```text
meta.json
summary.json
scores.json
tasks/<task-id>/r<repeat>/
  prompt.txt
  task.json
  ctx.json
  stream.jsonl
  workdir/
  score.json
```

Inspect or compare them with:

```bash
harness-evals report
harness-evals compare <run-a> <run-b>
```

Use multiple repeats before treating a model-dependent delta as stable. Never
hide `error` rows or count them as green; they mean the harness did not produce
enough valid evidence to judge the task.
