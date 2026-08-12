# Skills catalogue

As of the last ground-truth sweep, `~/lazar/skills/` holds **89 entries**
(`ls ~/lazar/skills | wc -l`): 62 general skills plus 27 `_meta/` operating
skills. This page is a map, not a replica — run `cat ~/lazar/skills/INDEX.md`
for the live one-line index.

## `_meta/` — operating basics

Skills about being lazar. These outrank other skills on conflict.

| Skill | Purpose |
|---|---|
| `memory` | Persistence across invocations; plain-markdown notes. |
| `load-context` | Bounded recipes for reading `logs/stream.jsonl` (never `cat`). |
| `subagent` | Delegation via `bin/lazar-spawn` / `pack-and-spawn.sh`. |
| `harness` | Ranked durable lessons (`harness.py add/load/helpful`). |
| `self-patch` | Fix your own recurring failures; draft kernel proposals, never self-apply. |
| `create-skill` | Scaffold a new skill correctly. |
| `heartbeat` | The `--tick` maintenance loop. |
| `log-harvest` | Distill the event stream into `memory/work-journal.md`. |
| `log-rotation` | Richer archive summaries after rotation. |
| `checkpoint` / `session-compression` | Session state snapshots and compression. |
| `health-check` / `first-run` / `session-startup` | Environment verification and bootstrapping. |
| `distill` / `distill-large` | Session distillation. |
| `propose-kernel-patch` | The only path for kernel changes (operator-applied). |
| `find-capability` / `semantic-recall` / `embed-corpus` | Search over skills and memory. |
| `archive-search` / `project-context` / `reviews` / `triage` / `ahe` / `loop` / `no-repeat-failed-calls` / `prune-baks` / `shell-handoff` / `skill-corpus-audit` / `session-end` / `create-tui` | Assorted maintenance and workflow skills. |

## Core behavioral skills

| Skill | Purpose |
|---|---|
| `verify` (VERIFY contract) | Grounding: prove claims with `[VERIFY]` blocks. |
| `guardrails` | Toggle sandbox / trusted zones. |
| `hooks-system` | Work with the hooks lifecycle. |
| `model` / `model-identity` / `model-tier` / `models` | Model selection and identity. |
| `provider-switch` / `provider-router` / `provider-failover` | Backend management (MiniMax, OpenRouter, ...). |
| `invoke-flags` / `env-vars` | CLI flags and environment reference. |
| `input-images` | Vision via image blocks. |

## Workflow skills (selection)

`agent-dispatch`, `agent-launcher`, `agent-sessions`, `deep-research`,
`research`, `code-search`, `repo-reviewer`, `architect-codebase-review`,
`reverse-engineering`, `edge-case-discovery`, `hallucination-risk-reviewer`,
`prompt-injection-auditor`, `skill-auditor`, `skill-recommender`,
`eval-and-testing`, `harness-evals`, `gauntlet-loop`, `meta-prompting`,
`engineering`, `dockerfile-best-practices`, `ffmpeg-workflow`,
`pandoc-workflow`, `browser-automation`, `desktop-control`,
`duckduckgo-search`, `obsidian-cli` / `obsidian-markdown` /
`obsidian-bases`, `json-canvas`, `pageindex`, `mcp` / `mcp-server`,
`lazar-status`, `lazartui`, `glazar`, `metrics`, `distill`,
`persistent-agent-memory`, plus vendored third-party packs
(`affaan-m-everything-claude-code`, `ComposioHQ-agent-orchestrator`,
`NousResearch-hermes-agent`, and others).

!!! note
    This catalogue drifts — skills are added constantly. The authoritative
    list is always `ls ~/lazar/skills/` and `cat ~/lazar/skills/INDEX.md`.
