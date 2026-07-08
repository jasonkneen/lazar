#!/usr/bin/env bash
# harvest.sh — turn raw logs into a durable, readable work journal so the
# multi-MB stream.jsonl archives become disposable.
#
# WHAT IT EXTRACTS (mechanical, no LLM — cheap + repeatable):
#   - per archive/current log: date range, the real user prompts (topics),
#     and the files lazar created/edited (from tool commands)
#   → appended to memory/work-journal.md as a dated section.
#
# WHAT IT DELIBERATELY DOESN'T TOUCH:
#   - memory/distilled/ (the LLM-distilled learnings — gotchas/recipes/etc).
#     Those are the JUDGMENT layer; the _meta/distill skill owns them. This
#     script never deletes or pattern-matches under memory/distilled/.
#     (Lesson 2026-07-08: distilled files are named <archive>.bak.md and a
#     naive `*.bak.*` cleanup ate two of them. Whitelist distilled, always.)
#
# IDEMPOTENT: records processed archives in memory/.harvested so re-runs only
# add new material. The current stream.jsonl is always re-scanned (it grows).
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
LOGS="$LAZAR_HOME/logs"
JOURNAL="$LAZAR_HOME/memory/work-journal.md"
MARKER="$LAZAR_HOME/memory/.harvested"
mkdir -p "$(dirname "$JOURNAL")"
touch "$MARKER"

[ -f "$JOURNAL" ] || printf '# Work journal — what lazar has built and worked on\n\nDated, harvested from logs by `_meta/log-harvest`. The raw stream.jsonl archives\nare disposable once harvested; this is the durable record. Newest first.\n\n' > "$JOURNAL"

# read_log <path>: cat or zcat depending on extension
read_log(){ case "$1" in *.gz) gzip -dc "$1" 2>/dev/null;; *) cat "$1" 2>/dev/null;; esac; }

# harvest_one <path> <label>: extract a dated section into a temp, echo it
harvest_one(){
  local path="$1" label="$2" data prompts files first_ts last_ts
  data="$(read_log "$path")"
  [ -z "$data" ] && return 1

  # Date range from ts_ms
  first_ts=$(printf '%s\n' "$data" | jq -R -rc 'fromjson? | .ts_ms // empty' 2>/dev/null | head -1)
  last_ts=$(printf '%s\n' "$data"  | jq -R -rc 'fromjson? | .ts_ms // empty' 2>/dev/null | tail -1)
  fmt_ts(){ [ -n "$1" ] && date -r $(( $1 / 1000 )) '+%Y-%m-%d %H:%M' 2>/dev/null || echo "?"; }

  # Real user prompts (drop nudges/heartbeat/experiment noise), deduped, capped
  # First line only (real prompts are one line; multi-line = assistant bleed),
  # skip nudges/heartbeat/noise, dedupe, cap length.
  prompts=$(printf '%s\n' "$data" \
    | jq -R -rc 'fromjson? | select(.kind=="user") | (.content // "" | split("\n")[0])' 2>/dev/null \
    | grep -v '^You said you would' | grep -v '^Your turn ended' \
    | grep -v '^Continue\. Use the bash' | grep -v '^run heartbeat' \
    | grep -v '^Read this state and add' | grep -v '^add one line to' \
    | grep -vE '^[[:space:]]' \
    | awk 'NF' | awk '!seen[$0]++' | head -40 \
    | sed 's/^/  - /; s/\(.\{140\}\).*/\1…/')

  # Files lazar created/edited under LAZAR_HOME zones (portable — not /Users/-only)
  files=$(printf '%s\n' "$data" \
    | jq -R -rc 'fromjson? | (.command // .requested_command // "") ' 2>/dev/null \
    | grep -oE "${LAZAR_HOME}/(workspace|memory|skills|hooks|src)/[^ \")'\''\`]*\.(html|css|js|ts|md|sh|py|go|rs|json)" \
    | grep -v '/logs/' | awk '!seen[$0]++' | head -80 | sed 's/^/  - /')

  [ -z "$prompts" ] && [ -z "$files" ] && return 1

  {
    printf '## %s  (%s → %s)\n\n' "$label" "$(fmt_ts "$first_ts")" "$(fmt_ts "$last_ts")"
    if [ -n "$prompts" ]; then printf '**Asked / worked on:**\n%s\n\n' "$prompts"; fi
    if [ -n "$files" ];   then printf '**Files touched:**\n%s\n\n' "$files"; fi
  }
}

NEW=0
# Oldest archives first so the journal reads chronologically when reversed.
for arc in $(ls -1 "$LOGS"/stream.jsonl.*.bak "$LOGS"/stream.jsonl.*.bak.gz 2>/dev/null | sort); do
  name=$(basename "$arc")
  grep -qxF "$name" "$MARKER" && continue          # already harvested
  section=$(harvest_one "$arc" "$name") || { echo "$name" >> "$MARKER"; continue; }
  # Prepend (newest-first): write section above existing body (after the header).
  { head -n 6 "$JOURNAL"; printf '%s\n' "$section"; tail -n +7 "$JOURNAL"; } > "$JOURNAL.tmp" && mv "$JOURNAL.tmp" "$JOURNAL"
  echo "$name" >> "$MARKER"
  NEW=$((NEW+1))
done

# Always (re)harvest the live log into a rolling "current" section at the top.
CUR=$(harvest_one "$LOGS/stream.jsonl" "current stream.jsonl")
if [ -n "$CUR" ]; then
  # Drop any previous "current" section, then prepend the fresh one.
  awk 'BEGIN{skip=0} /^## current stream\.jsonl/{skip=1} skip==1 && /^## / && !/^## current stream/{skip=0} skip==0{print}' "$JOURNAL" > "$JOURNAL.tmp" 2>/dev/null || cp "$JOURNAL" "$JOURNAL.tmp"
  { head -n 6 "$JOURNAL.tmp"; printf '%s\n' "$CUR"; tail -n +7 "$JOURNAL.tmp"; } > "$JOURNAL" && rm -f "$JOURNAL.tmp"
fi

echo "harvested $NEW new archive(s); journal: $JOURNAL ($(wc -l < "$JOURNAL") lines)"
