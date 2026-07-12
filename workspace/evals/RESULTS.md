# Eval results — one-tool Rust kernel + skills

## 2026-07-12: event-level cross-provider grounding baseline

The model compatibility gate now consumes `--output-format stream-json` and
requires observable protocol events. A passing result needs:

1. a genuine `tool_use` event;
2. its matching `tool_result` containing the run token;
3. the expected artifact on disk;
4. a passing kernel `verify_report`; and
5. no flagged `fabrication_check` event.

Merely repeating the command or success token in assistant prose cannot pass.

| Model | Exact-command protocol | Inspect/edit/run-verifier workflow |
|---|---:|---:|
| `MiniMax-M3` | **FAIL** — verification missing | **FAIL** — verification missing |
| `anthropic/claude-haiku-4.5` | PASS | PASS |
| `openai/gpt-5.1` | PASS | PASS |
| `google/gemini-3.5-flash` | PASS | PASS |

MiniMax-M3 emitted genuine tool calls and produced the artifacts in this run,
but omitted the explicitly required `[VERIFY]` block in both scenarios. The
other three models completed the tool protocol, grounded workflow, and kernel
verification end to end.

Run this gate directly:

```bash
bash ~/lazar/workspace/evals/model-evals.sh
EVAL_MODELS="MiniMax-M3 anthropic/claude-haiku-4.5" \
  bash ~/lazar/workspace/evals/model-evals.sh
bash ~/lazar/workspace/evals/model-evals.sh --self-test
```

The results below are the earlier text-oriented baseline. Those checks remain
useful for broad agent capabilities, but their token-grep tool test does not
prove that a real tool call occurred; use `model-evals.sh` for model grounding.

**Date:** 2026-07-08  
**Stack:** `bin/lazar` (single `bash` tool) + `skills/` markdown + Keychain keys  
**Endpoint:** MiniMax native Anthropic (`https://api.minimax.io/anthropic`)  
**Thesis:** Models can do real work with only one tool if skills + VERIFY exist; TUI is optional chrome.

## Offline gate

| Suite | Result |
|-------|--------|
| Kernel unit tests | 72/72 |
| Skill selftests (subagent, log-harvest, self-patch, env-vars) | PASS |
| TUI go tests | PASS |

## Live suites

### Agent evals (`agent-evals.sh`) — thesis suite

| # | Check | M3 | M2.5-hs | M2.7 |
|---|--------|----|---------|------|
| A1 | Single bash tool executes | PASS | PASS | PASS |
| A2 | Multi-step tool loop → real files | PASS | PASS | PASS |
| A3 | VERIFY-backed claim on disk | PASS | PASS | PASS |
| A4 | Skill discovery via INDEX (bash-read) | PASS | PASS | PASS |
| A5 | Follow env-vars skill recipe | PASS | PASS | PASS |
| A6 | Instructed write boundary | PASS | PASS | PASS |
| A7 | Subagent skill live-fire | PASS | PASS | PASS |

### Kernel evals (`kernel-evals.sh`) — hardened behaviors

| Check | M3 | M2.5-hs | M2.7 |
|-------|----|---------|------|
| Tool exec | PASS | PASS | PASS |
| VERIFY task | PASS | PASS | PASS |
| Big-write chunking (~47KB) | PASS | PASS | PASS |
| Subagent live-fire | PASS | PASS | PASS |
| Spawn veto / allow / decoy | PASS | PASS | PASS |

## How to re-run

```bash
source ~/lazar/scripts/lazar-env.sh
bash ~/lazar/workspace/evals/run-all.sh              # offline + all live models
bash ~/lazar/workspace/evals/agent-evals.sh          # thesis suite only (default MiniMax-M3)
EVAL_MODEL=MiniMax-M2.7 bash ~/lazar/workspace/evals/agent-evals.sh
```

## Readout

On MiniMax M3 / M2.5-highspeed / M2.7, the **minimal kernel is sufficient**: models use the one tool, read skills from disk, honor VERIFY, and can spawn through the sanctioned subagent skill. No TUI required for the agent loop.
