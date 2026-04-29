# load-context

Recover relevant prior conversation without blowing your context window. Lazar has **three tiers of memory** — work through them in order, only going deeper when shallower tiers don't have the answer:

- **L1** — `$LAZAR_HOME/logs/stream.jsonl` (current, raw events). This skill's main subject.
- **L2** — `$LAZAR_HOME/memory/log-summaries/*.md` (durable summaries written at rotation by `_meta/log-rotation`). Read these when L1 doesn't go back far enough.
- **L3** — `$LAZAR_HOME/logs/stream.jsonl.*.bak` (full forensic archives). Use `_meta/archive-search` to query these. Last resort.

Every read is bounded. The log and archives can each be megabytes — `cat`-ing the whole thing will fail with `prompt is too long`.

## When to use

- The user's prompt is referential ("yes", "do that", "continue", "as I said", "what we discussed").
- You're about to do something that would benefit from prior context (e.g. resuming a multi-step task).
- The user mentions a skill, file, or decision you don't remember.

## Hard rules

- **NEVER** `cat $LAZAR_HOME/logs/stream.jsonl`. The log is unbounded; `cat` will exceed the API context window and the entire turn fails.
- ALWAYS bound your read by both line count (`-n`) AND bytes (`-c`).
- Start narrow. Widen only if you didn't find what you need.

## Recipes

### Default move: last N events, byte-capped

    tail -n 200 $LAZAR_HOME/logs/stream.jsonl | tail -c 50000

Use this 90% of the time. The double-tail bounds both lines and bytes against pathological log entries (e.g. one giant tool_result).

### Last N user prompts only (lightweight context check)

    grep '"kind":"user"' $LAZAR_HOME/logs/stream.jsonl | tail -n 5 | tail -c 20000

### The most recent invocation (start to finish)

    awk '/"kind":"invoke_start"/{n++; if(n>1) out=""} {out=out $0 "\n"} END{print out}' \
      $LAZAR_HOME/logs/stream.jsonl | tail -c 80000

### Search for a topic across all history

    grep -i "topic-name" $LAZAR_HOME/logs/stream.jsonl | tail -n 50 | tail -c 50000

### Filter by event kind

    jq -r 'select(.kind == "tool_result") | .command' $LAZAR_HOME/logs/stream.jsonl \
      | tail -n 30 \
      | tail -c 20000

## When L1 isn't enough — go to L2 (memory summaries + distilled)

L2 has two flavors, both under `$LAZAR_HOME/memory/`:

**Mechanical summaries** at `memory/log-summaries/<ts>.md`, written automatically by `_meta/log-rotation`. Each one tells you what time period a `.bak` archive covers, the top user prompts, and the most-used commands. Cheap, broad, always present.

    ls -la $LAZAR_HOME/memory/log-summaries/
    cat $LAZAR_HOME/memory/log-summaries/*.md 2>/dev/null | head -n 100 | head -c 50000

**Distilled learnings** at `memory/distilled/<category>.md` (gotchas / conventions / recipes / preferences), written by `_meta/distill` when the agent has been asked to curate. High-signal, but only present if `distill` has been run for the archive in question. Read these FIRST when looking for "what did we learn" rather than "what happened":

    cat $LAZAR_HOME/memory/distilled/*.md 2>/dev/null | head -n 200 | head -c 50000

Use the summaries to figure out *which* archive is worth searching, and the distilled files to recall lessons without re-reading transcripts.

## When L2 points at an archive — go to L3 (`_meta/archive-search`)

If a summary indicates the relevant history is in a specific `.bak` file, read `_meta/archive-search/SKILL.md` for safe per-archive search recipes. **Do not** try to grep all archives in one go — that's how you blow context.

## Principle

The log is your infinite memory. Treat it like a database, not a notebook — query, don't dump. If the same load pattern keeps coming up, document it as a new skill or extend this one.

## Failure mode

If your bounded read still produces too much (e.g. 200 lines but each is huge), narrow further:

    tail -n 50 $LAZAR_HOME/logs/stream.jsonl | tail -c 20000

If the user's prompt is genuinely referring to a session that is older than what `tail` can show, ask them what they meant rather than scanning the entire archive.
