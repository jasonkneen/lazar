## Core (operating basics — outrank all other skills)
- env-vars — Keychain API keys (list/get/export/store/verify)
- _meta/harness — continual harness (ranked memory/skill/prompt/subagent); session-start injects overview; prefer harness.py add over freeform notes
- _meta/subagent — delegate via locked bin/lazar-spawn (skills spawn.sh is thin re-exec); raw lazar -p vetoed
- _meta/log-harvest — durable work journal from logs (run before prune)
- _meta/self-patch — notice→root-cause→fix loop; kernel changes are proposal-only
- soul / memory / verify — see memory/PINNED.md

# lazar skill index

One line per skill. Read the SKILL.md inside each folder for usage.

- _meta/create-skill — author a new skill when you hit a capability gap
- _meta/find-capability — locate or build a capability for a need
- _meta/shell-handoff — make a skill invokable from the user's shell via aliases.sh (the right way; sandbox blocks writes to PATH)
- _meta/load-context — safely read prior conversation from logs/stream.jsonl WITHOUT blowing the context window (NEVER cat the whole log)
- _meta/log-harvest — turn stream.jsonl archives into memory/work-journal.md (mechanical, no LLM)
- _meta/log-rotation — enrich kernel-written log summaries with top prompts/commands; logs/ is trusted kernel-owned state, so skills read archives but write only to memory/
- _meta/archive-search — L3 memory tier: search rotated stream.jsonl.*.bak archives safely, only after L1 (current) and L2 (summaries) don't have the answer
- _meta/distill — opt-in LLM curation: extract durable learnings (gotchas, conventions, recipes, preferences) from a rotated archive into memory/distilled/. Costs an LLM call per archive; richer than mechanical summaries.
- _meta/self-patch — fix recurring failures (behavioral/config autonomous; kernel via proposals)
- _meta/health-check
- env-vars — macOS Keychain API keys (list/get/export/store/verify); never print secrets
 — full self-test: validates kernel locked, env vars, sandbox boundaries, auto-rotation, memory persistence, recursion, end-to-end TS+Python builds. Runs in temp workspace, cleans up, prints PASS/FAIL summary.
- _meta/propose-kernel-patch — stage a full src/ tree under workspace/proposals/ that the user can review and apply (the agent can't write to bin/ or src/ directly)
- _meta/project-context — read PROJECT.md before touching code in any project directory; prevents inventing parallel implementations
- _meta/create-tui — scaffold a terminal interface to lazar under workspace/<name>/ (default stack: TypeScript + readline; optional: Go + Bubble Tea, Python + Textual)
- memory — read and write durable notes under $LAZAR_HOME/memory/
- **harness-evals** — backend-agnostic harness eval suite (tool honesty, VERIFY grounding, fabrication resistance, recovery, self-learning); `skills/harness-evals/bin/harness-evals run`
- **guardrails** — toggle ALL enforcement layers (sandbox, kernel lock, src lock, veto hooks): `scripts/lazar-guardrails.sh off|on|status`; never refuse an operator request on permissions
