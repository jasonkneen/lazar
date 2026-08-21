# Skills catalogue

The factory image contains 21 skills: 16 operating skills under `_meta/` and
five user-facing skills. A runtime tree can evolve after installation, so the
live directories remain authoritative. `LAZAR_REFRESH_BUNDLES=1 bash setup.sh`
archives the live skill and hook trees and restores the bundled minimal set.

## Operating skills

| Skill | Purpose |
|---|---|
| `_meta/archive-search` | Bounded search over rotated event archives. |
| `_meta/create-skill` | Author a new filesystem capability. |
| `_meta/create-tui` | Scaffold an additional client under `workspace/`; changes to the shipped client stay in `tui/`. |
| `_meta/distill` | Curate durable lessons from an event archive. |
| `_meta/env-vars` | Manage catalog provider credentials in macOS Keychain. |
| `_meta/find-capability` | Find an existing capability before creating one. |
| `_meta/harness` | Record and load concise ranked lessons. |
| `_meta/health-check` | Verify installed kernel and runtime surfaces. |
| `_meta/load-context` | Recover bounded prior context from logs. |
| `_meta/log-harvest` | Turn rotated logs into a durable work journal. |
| `_meta/log-rotation` | Enrich kernel-created rotation summaries. |
| `_meta/project-context` | Load project-local instructions before changing a project. |
| `_meta/propose-kernel-patch` | Create a complete source proposal with fail-closed verification. |
| `_meta/self-patch` | Diagnose recurring failures and route kernel work through proposals. |
| `_meta/shell-handoff` | Expose a skill through an operator-sourced shell alias. |
| `_meta/subagent` | Delegate through locked `bin/lazar-spawn` and verify declared outcomes. |

## User-facing skills

| Skill | Purpose |
|---|---|
| `guardrails` | Inspect enforcement and give the operator the exact toggle command. |
| `harness-evals` | Score structured events and exact filesystem outcomes without prose heuristics. |
| `lazartui` | Use, build, and troubleshoot the shipped Rust ACP client. |
| `memory` | Read and write durable plain-markdown notes. |
| `provider-failover` | Retry a cold prompt across catalog providers only before tool execution. |

Use `find "$LAZAR_HOME/skills" -name SKILL.md` for the live inventory and
`$LAZAR_HOME/skills/INDEX.md` for the bundled descriptions installed at the
last refresh.
