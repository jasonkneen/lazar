# Eval results — one-tool Rust kernel + skills

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
