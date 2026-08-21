# lazar bundled skill index

One line per bundled skill. Read the `SKILL.md` inside each folder before use.
Runtime skills may evolve after installation; `LAZAR_REFRESH_BUNDLES=1 bash
setup.sh` archives the skill and hook trees and restores the bundled set.

## Core operating skills

- _meta/env-vars — manage catalog provider keys in macOS Keychain without printing secrets
- _meta/harness — record and load concise ranked lessons
- _meta/subagent — delegate through locked `bin/lazar-spawn` with verifiable claims
- _meta/load-context — recover bounded prior context from logs
- _meta/log-harvest — turn rotated logs into a durable work journal
- _meta/self-patch — diagnose recurring failures; stage kernel changes for operator review
- memory — read and write durable plain-markdown notes

## Maintenance and extension

- _meta/archive-search — bounded search over rotated event archives
- _meta/create-skill — author a capability as a new skill
- _meta/create-tui — scaffold an additional client under workspace; shipped-client work stays in `tui/`
- _meta/distill — curate durable lessons from a rotated archive
- _meta/find-capability — locate an existing capability before creating one
- _meta/health-check — verify the installed kernel and runtime surfaces
- _meta/log-rotation — enrich kernel-created rotation summaries
- _meta/project-context — load a project's own instructions before changing it
- _meta/propose-kernel-patch — create a complete fail-closed source proposal
- _meta/shell-handoff — expose a skill through an operator-sourced shell alias
- guardrails — inspect enforcement and give the operator the exact toggle command
- harness-evals — score structured events and filesystem outcomes without prose heuristics
- lazartui — use, build, and troubleshoot the shipped Rust ACP client
- provider-failover — retry a cold prompt across catalog providers only before tool execution
