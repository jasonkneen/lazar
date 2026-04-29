# lazar skill index

One line per skill. Read the SKILL.md inside each folder for usage.

- _meta/create-skill — author a new skill when you hit a capability gap
- _meta/find-capability — locate or build a capability for a need
- _meta/shell-handoff — make a skill invokable from the user's shell via aliases.sh (the right way; sandbox blocks writes to PATH)
- _meta/load-context — safely read prior conversation from logs/stream.jsonl WITHOUT blowing the context window (NEVER cat the whole log)
- _meta/log-rotation — kernel auto-rotates at LAZAR_LOG_MAX_BYTES; this skill is for richer per-archive summaries (top prompts/commands), compression, and pruning
- _meta/archive-search — L3 memory tier: search rotated stream.jsonl.*.bak archives safely, only after L1 (current) and L2 (summaries) don't have the answer
- _meta/distill — opt-in LLM curation: extract durable learnings (gotchas, conventions, recipes, preferences) from a rotated archive into memory/distilled/. Costs an LLM call per archive; richer than mechanical summaries.
- _meta/health-check — full self-test: validates kernel locked, env vars, sandbox boundaries, auto-rotation, memory persistence, recursion, end-to-end TS+Python builds. Runs in temp workspace, cleans up, prints PASS/FAIL summary.
- _meta/propose-kernel-patch — stage a full src/ tree under workspace/proposals/ that the user can review and apply (the agent can't write to bin/ or src/ directly)
- _meta/project-context — read PROJECT.md before touching code in any project directory; prevents inventing parallel implementations
- _meta/create-tui — scaffold a terminal interface to lazar under workspace/<name>/ (default stack: TypeScript + readline; optional: Go + Bubble Tea, Python + Textual)
- memory — read and write durable notes under $LAZAR_HOME/memory/
