# archive-search

Search across rotated log archives (`$LAZAR_HOME/logs/stream.jsonl.*.bak`) when you can't find what you need in current `stream.jsonl` or in `memory/log-summaries/`.

This is the **L3 tier** of lazar's three-tier memory. Use it when L1 (current log) and L2 (summaries in memory) don't have the answer. It is never the *first* move — by the time you reach for archives you should already know roughly which time window to look in, because L2 summaries tell you the date range covered by each `.bak` file.

## When to use

- The user references a session, command, or decision older than what's in current `stream.jsonl`.
- L2 summaries (`cat $LAZAR_HOME/memory/log-summaries/*.md`) point at a specific archive.
- You're doing forensic recall of a specific tool call, exact prompt, or full transcript from a past invocation.

## Hard rules

- **NEVER** `cat $LAZAR_HOME/logs/stream.jsonl.*.bak`. Archives can be 10MB each — globbing them all into one cat will absolutely blow context.
- **Always grep with bounds.** Process one archive at a time, pipe through `tail -c` or `head -n`.
- **Read L2 first.** Each `.bak` archive has a sibling summary at `memory/log-summaries/<ts>.md`. Read those to figure out *which* archive to search before touching one.

## Recipes

### Decide which archive to search (do this first)

    ls -la $LAZAR_HOME/memory/log-summaries/
    cat $LAZAR_HOME/memory/log-summaries/*.md | head -n 100   # bounded scan of all summaries

### List archives by age

    ls -lat $LAZAR_HOME/logs/stream.jsonl.*.bak

The filename embeds a unix timestamp: `stream.jsonl.<ts>.bak`. Bigger number = newer.

### Search ONE archive for a topic

    grep -i "topic-keyword" $LAZAR_HOME/logs/stream.jsonl.<TS>.bak | tail -n 50

### Find a specific user prompt across all archives (bounded)

    for f in $LAZAR_HOME/logs/stream.jsonl.*.bak; do
      jq -r 'select(.kind == "user") | "\(.ts_ms) \(.content)"' "$f" 2>/dev/null \
        | grep -i "your-search" \
        | tail -n 5
    done | tail -n 30

### Find when a particular file was first touched

    for f in $LAZAR_HOME/logs/stream.jsonl.*.bak $LAZAR_HOME/logs/stream.jsonl; do
      grep -l "/path/to/file" "$f" 2>/dev/null
    done | head -n 1

### Pull the full context of a specific invocation by ts_ms range

If you know an invocation happened around timestamp `T`, slice the relevant archive:

    awk -v lo=$T -v hi=$((T+60000)) '
      /"ts_ms"/ {
        match($0, /"ts_ms":[0-9]+/)
        ts = substr($0, RSTART+8, RLENGTH-8) + 0
        if (ts >= lo && ts <= hi) print
      }
    ' $LAZAR_HOME/logs/stream.jsonl.<TS>.bak | tail -c 50000

### Compressed archives

If `_meta/log-rotation` step "Compress old archives" was run, archives end in `.bak.gz`. Use `zgrep` and `zcat`:

    zgrep -i "topic" $LAZAR_HOME/logs/stream.jsonl.<TS>.bak.gz | tail -n 50
    zcat $LAZAR_HOME/logs/stream.jsonl.<TS>.bak.gz | jq -r 'select(.kind == "user") | .content' | tail -n 30

## Workflow when the user asks about old context

1. `cat $LAZAR_HOME/memory/log-summaries/*.md | head -n 100` — see what time periods are summarized.
2. Identify the right summary by date or topic. Note its archive filename (in the summary header).
3. Grep that single archive with bounded output.
4. If multiple archives might contain it, iterate over them ONE AT A TIME with `for f in ...; do ... done` and tail the cumulative output.

## Anti-patterns

- Globbing all archives into a single `cat` or `jq`. Always per-file with bounds.
- Using `grep -r` without piping into `head` or `tail` — recursive grep across 100MB of archives is slow.
- Reading archives when L2 summaries already answered the question.
- Forgetting to read summaries first and going straight to archive search.

## Pairing

- `_meta/log-rotation` writes the L2 summaries you read first.
- `_meta/load-context` is the entry point — it points here only when L1 and L2 don't suffice.
- `memory` skill — if you find something genuinely durable in an archive (a key fact, a recurring command, a decision), promote it from the archive to a regular memory note so you don't have to rediscover it next time.
