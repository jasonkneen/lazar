# log-rotation

Add a *richer* summary to a log archive — top user prompts, top tool commands, file-touch index — beyond what the kernel writes automatically. Optionally compress old archives and prune very old ones.

**The kernel auto-rotates `stream.jsonl` when it exceeds `LAZAR_LOG_MAX_BYTES` (default 10MB).** That's the safety floor — you do NOT need this skill to keep the log from breaking the agent. The kernel writes a minimal summary (archive name, size, timestamp) into `memory/log-summaries/<ts>.md` whenever it rotates.

This skill is for the polish layer on top: when you want a per-archive summary that includes top-30 user prompts and top-20 tool commands, or you want to compress/prune archives.

## When to use

- After kernel auto-rotation, if you want richer summaries than the minimal kernel header.
- When you want to compress old `.bak` archives to save disk.
- When you want to prune the oldest archives (>5 in keep-list).
- The user explicitly asks "summarize the last session" or similar.

Do NOT run this on every prompt. Once-per-archive is plenty. The kernel's auto-rotation keeps the log safe regardless.

## Hard rules

- Rotation itself is non-destructive — moves the live log to `.bak`, never deletes.
- `mv` is atomic on the same filesystem — safe under concurrent writes. The next `append_stream` call from the kernel creates a fresh empty `stream.jsonl`.
- Don't manually rotate just for the sake of it. Let the kernel handle the size threshold; this skill enriches archives, it doesn't replace the floor.

## Recipes

### Check size (cheap, do this first)

    LOG=$LAZAR_HOME/logs/stream.jsonl
    [ -f "$LOG" ] && wc -c "$LOG"

### Rotate if over threshold (and write a summary to memory/)

    LOG="$LAZAR_HOME/logs/stream.jsonl"
    THRESHOLD=10485760  # 10 MB

    if [ -f "$LOG" ]; then
      SIZE=$(stat -f%z "$LOG" 2>/dev/null || stat -c%s "$LOG" 2>/dev/null || echo 0)
      if [ "${SIZE:-0}" -gt "$THRESHOLD" ]; then
        TS="$(date +%s).$$"
        ARCHIVE="${LOG}.${TS}.bak"
        N=0
        while [ -e "$ARCHIVE" ]; do
          N=$((N+1))
          ARCHIVE="${LOG}.${TS}.${N}.bak"
        done
        mv "$LOG" "$ARCHIVE"
        echo "rotated: ${LOG} -> ${ARCHIVE} (was ${SIZE} bytes)"

        # CRITICAL: write a summary into memory/ so long-term recall stays cheap.
        # This is the L2 tier — the agent reads these instead of scanning .bak files.
        mkdir -p "$LAZAR_HOME/memory/log-summaries"
        SUM="$LAZAR_HOME/memory/log-summaries/$(basename "$ARCHIVE").md"
        FIRST_TS=$(head -n 1 "$ARCHIVE" | jq -r '.ts_ms // empty' 2>/dev/null || echo "?")
        LAST_TS=$(tail -n 1  "$ARCHIVE" | jq -r '.ts_ms // empty' 2>/dev/null || echo "?")
        N_INVOKES=$(grep -c '"kind":"invoke_start"' "$ARCHIVE" 2>/dev/null || echo 0)
        N_TOOLS=$(grep -c   '"kind":"tool_result"'  "$ARCHIVE" 2>/dev/null || echo 0)

        cat > "$SUM" <<EOF
# log-summary $(date -u -r "${TS%%.*}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date +%Y-%m-%d)

archive: $ARCHIVE
size: ${SIZE} bytes
first_ts_ms: ${FIRST_TS}
last_ts_ms: ${LAST_TS}
invocations: ${N_INVOKES}
tool_calls: ${N_TOOLS}

## User prompts (last 30)

EOF
        jq -r 'select(.kind == "user") | (.content | tostring | .[0:1000])' "$ARCHIVE" 2>/dev/null \
          | tail -n 30 \
          | sed 's/^/- /' \
          >> "$SUM" || true

        echo "" >> "$SUM"
        echo "## Tool commands (top 20 by frequency)" >> "$SUM"
        echo "" >> "$SUM"
        jq -r 'select(.kind == "tool_result") | .command' "$ARCHIVE" 2>/dev/null \
          | awk '{print $1}' \
          | sort | uniq -c | sort -rn \
          | head -n 20 \
          | sed 's/^/    /' \
          >> "$SUM" || true

        echo "summary: $SUM"
      else
        echo "no rotation needed (${SIZE:-0} bytes, threshold ${THRESHOLD})"
      fi
    fi

### Compress old archives (optional, saves disk)

    for f in $LAZAR_HOME/logs/stream.jsonl.*.bak; do
      [ -f "$f" ] && [ ! -f "${f}.gz" ] && gzip "$f"
    done

### Prune very old archives (optional, conservative — keeps last 5)

    find "$LAZAR_HOME/logs" -name 'stream.jsonl.*.bak*' -type f -print \
      | while IFS= read -r f; do printf '%s\t%s\n' "$(stat -f%m "$f" 2>/dev/null || stat -c%Y "$f" 2>/dev/null || echo 0)" "$f"; done \
      | sort -rn \
      | cut -f2- \
      | tail -n +6 \
      | while IFS= read -r old; do rm -f -- "$old"; done

## Pairing with load-context and archive-search

The three tiers of memory:

- **L1**: `$LAZAR_HOME/logs/stream.jsonl` (current, raw) — bounded `tail`/`grep` via `_meta/load-context`.
- **L2**: `$LAZAR_HOME/memory/log-summaries/<ts>.md` (durable summaries this skill writes at rotation) — read these first when looking back, they tell you which archive to dig into.
- **L3**: `$LAZAR_HOME/logs/stream.jsonl.*.bak` (full forensic record) — searched via `_meta/archive-search` only when L1+L2 don't have what's needed.

Never `cat` an archive. Always grep with bounds. If you compress archives with gzip, note it in memory so future-you uses `zgrep`/`zcat`.

## Principle

The kernel just records. Hygiene is the agent's responsibility, expressed as this skill. Treat rotation like rotating syslog or nginx access logs — boring infrastructure, but boringly reliable.
