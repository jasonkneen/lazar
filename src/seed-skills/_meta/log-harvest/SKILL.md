---
name: log-harvest
description: Turn raw stream.jsonl logs into durable, readable memory — a work journal of what I built and worked on — so the multi-MB archives stop being the only record of my own history. Run after sessions, before pruning logs, or whenever I can't recall recent work. Pairs with _meta/distill (the learnings layer). Use when logs are large, when I've lost track of what I've done, or when preparing to prune/compress the log archives.
---

# log-harvest — make my own history durable and searchable

Raw `logs/stream.jsonl` (+ rotated `.bak`/`.bak.gz` archives) is where my real history lives, but it's multi-MB JSONL I can't hold in context. If I lose it, I lose the memory of my own work (on 2026-07-08 I told the operator a real page they built "didn't exist"). This skill harvests the logs into a durable record so that stops happening.

## Two layers — both matter

1. **Mechanical harvest (this skill, `harvest.sh`)** — cheap, no LLM, repeatable. Extracts per archive: date range, the real user prompts (topics), and files I created/edited → `memory/work-journal.md`. This is my **navigation index**: `rg <topic> memory/work-journal.md` to find when/what.
2. **LLM distillation (`_meta/distill`)** — judgment, costs a call per archive. Extracts gotchas/recipes/conventions/preferences → `memory/distilled/`. This is my **learnings** layer.

The logs are only safely disposable once BOTH have run over them.

## Run it

    bash skills/_meta/log-harvest/harvest.sh

Idempotent — a marker (`memory/.harvested`) records processed archives; re-runs only add new material. The live `stream.jsonl` is always re-scanned (it grows).

## HONEST LIMITS (evaluated 2026-07-08 — do not oversell this)

- **Mechanical harvest is LOSSY.** It caps prompts/files per section and its file-path regex misses some writes (it missed a real `pricing-component/index.html`). It is a *navigation index*, NOT a lossless archive.
- **Therefore: do NOT fully delete raw logs after harvesting.** The right disposability protocol is:
  - **Compress, don't delete.** `gzip` any uncompressed `stream.jsonl.*.bak` (rotated archives are already superseded in the live log). Gzipped archives are cheap cold storage and the ground-truth fallback.
  - Keep `memory/work-journal.md` + `memory/distilled/` as the hot layer I actually read.
  - Only delete a raw archive if disk genuinely forces it AND it's both harvested and distilled — and even then prefer keeping the gzip.
- **NEVER pattern-delete under `memory/distilled/`.** Distilled files are named `<archive>.bak.md`, so a naive `*.bak.*` cleanup eats real learnings (I did this to two files on 2026-07-08). Whitelist `memory/distilled/` in any cleanup.

## Self-test

    bash skills/_meta/log-harvest/selftest.sh
