# log-rotation

Add a richer memory summary for a rotated Lazar stream archive.

**The kernel auto-rotates `stream.jsonl` when it exceeds `LAZAR_LOG_MAX_BYTES` (default 10MB).** That is the safety floor. The kernel owns `logs/`: sandboxed tools and hooks may read logs, but must not write, compress, prune, or repair them.

The kernel writes a minimal summary into `memory/log-summaries/<ts>.md` whenever it rotates. This skill is the polish layer on top: read one archive with bounded commands and append useful human navigation details to memory.

## When to use

- After kernel auto-rotation, if you want richer summaries than the minimal kernel header.
- When a `memory/log-summaries/<ts>.md` file exists but needs top prompts, top commands, or a small file-touch index.
- The user explicitly asks "summarize the last session" or similar.

Do NOT run this on every prompt. Once per archive is plenty.

## Hard rules

- `logs/` is trusted kernel-owned state. Do not `mv`, `gzip`, `rm`, truncate, or write anything under `$LAZAR_HOME/logs` from a sandboxed skill or hook.
- Rotation itself is kernel-owned and non-destructive at the moment it happens: the live log moves to `.bak`, and the next append creates a fresh `stream.jsonl`.
- Full-fidelity archive retention is kernel policy, controlled by `LAZAR_LOG_ARCHIVE_KEEP`. Summaries in `memory/log-summaries/` are the durable navigation layer.
- Write enriched summaries only under `$LAZAR_MEMORY`, usually `memory/log-summaries/` or `memory/distilled/`.
- Always bound archive reads. Never `cat` a whole archive.

## Recipes

### Check current stream size

    LOG=$LAZAR_HOME/logs/stream.jsonl
    [ -f "$LOG" ] && wc -c "$LOG"

### Pick the newest rotated archive

    ARCHIVE=$(ls -t $LAZAR_HOME/logs/stream.jsonl.*.bak 2>/dev/null | head -n 1)
    [ -n "$ARCHIVE" ] || { echo "no raw archive found"; exit 0; }

If an archive has already been pruned by the kernel, use the matching `memory/log-summaries/<ts>.md` summary as the durable record.

### Enrich the summary for one archive

    ARCHIVE=$LAZAR_HOME/logs/stream.jsonl.<TS>.bak
    TS=$(basename "$ARCHIVE" .bak | sed 's/^stream\.jsonl\.//')
    SUM=$LAZAR_HOME/memory/log-summaries/${TS}.md
    mkdir -p "$LAZAR_HOME/memory/log-summaries"

    {
      echo ""
      echo "## User prompts (last 30)"
      echo ""
      jq -r 'select(.kind == "user") | .content' "$ARCHIVE" 2>/dev/null \
        | tail -n 30 \
        | sed 's/^/- /' \
        || true

      echo ""
      echo "## Tool commands (top 20 by frequency)"
      echo ""
      jq -r 'select(.kind == "tool_result") | .command' "$ARCHIVE" 2>/dev/null \
        | awk '{print $1}' \
        | sort | uniq -c | sort -rn \
        | head -n 20 \
        | sed 's/^/    /' \
        || true
    } >> "$SUM"

    echo "summary: $SUM"

## Pairing with load-context and archive-search

The three tiers of memory:

- **L1**: `$LAZAR_HOME/logs/stream.jsonl` (current, raw) — bounded `tail`/`rg` via `_meta/load-context`.
- **L2**: `$LAZAR_HOME/memory/log-summaries/<ts>.md` — minimal entries written by the kernel, optionally enriched by this skill.
- **L3**: `$LAZAR_HOME/logs/stream.jsonl.*.bak` (recent full-fidelity archives) — searched via `_meta/archive-search` only when L1+L2 don't have what's needed. Kernel retention may prune old L3 archives.

Never `cat` an archive. Always use `rg`, `jq`, `head`, or `tail` with explicit bounds.

## Principle

The kernel records and rotates. Skills interpret and summarize. Keep it that way: no log mutation from sandboxed skills.
