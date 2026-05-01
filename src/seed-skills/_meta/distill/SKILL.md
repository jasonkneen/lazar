# distill

Use lazar recursion to extract durable, structured learnings from rotated log archives, and write them into categorized memory files. Turns raw events (L3) into curated knowledge (a richer L2) on demand.

This is the **opt-in LLM curation** layer for lazar's memory. The mechanical L2 summaries written by `_meta/log-rotation` give you "what topics came up and what commands ran." `distill` goes further and asks the agent itself: "what did we learn that's worth remembering?"

## When to use

- After a busy session has rotated, when you want curated learnings (not just a transcript) preserved.
- The user explicitly asks the agent to "remember what we figured out" or "save the lessons from this work."
- Periodically — say once per `.bak` archive — to build up a knowledge base over time.

Do NOT use this on every invocation. It costs an LLM call per archive, and learnings stabilize quickly. Once per rotation is plenty.

## Hard rules

- **Bound the sample.** Never pass a whole archive into the recursion prompt. Pick a focused slice — last N invocations, or events around a specific topic — and cap by bytes.
- **Use `lazar -p` recursion**, not a separate API call. This keeps the kernel as the only API consumer and makes the LLM call appear in `stream.jsonl` itself.
- **Write to `memory/distilled/<category>.md`**, never to top-level memory. Distilled insights are LLM-generated; mark them clearly.
- **Tag every entry** with the source archive's timestamp and the date you distilled. Future-you needs to know how stale the learning is.

## Recipe

```bash
# Pick the archive to distill (default: most recent rotation)
ARCHIVE=$(ls -t $LAZAR_HOME/logs/stream.jsonl.*.bak 2>/dev/null | head -n 1)
[ -z "$ARCHIVE" ] && { echo "no archives to distill"; exit 0; }

# Bounded sample: last 30 user prompts + assistant responses, capped at 30KB
SAMPLE=$(jq -s '
  map(select(.kind == "user" or .kind == "assistant"))
  | .[-30:]
  | map({kind: .kind, content: (.content | tostring | .[0:2000])})
' "$ARCHIVE" 2>/dev/null | head -c 30000)

[ -z "$SAMPLE" ] && { echo "no usable events in $ARCHIVE"; exit 0; }

# Recurse — ask lazar to extract structured learnings
LAZAR_DEPTH=1 lazar -p "Read this conversation transcript (JSONL events) and extract durable learnings worth saving. Be selective — only include things future-you will genuinely benefit from remembering.

Output ONLY this exact format, with each section terse and one-line bullets:

## gotchas
- <thing that bit us, prefixed with the situation>

## conventions
- <a code/style/path convention adopted in this work>

## recipes
- <a multi-step bash/tool pattern that worked>

## preferences
- <a stated user preference>

Skip any section that has nothing real. Don't pad. If nothing is worth saving, output 'nothing durable' on a single line.

Transcript:
$SAMPLE" > /tmp/distill-out.md 2>/dev/null

# If the agent said nothing durable, bail
grep -qi "nothing durable" /tmp/distill-out.md && { echo "no durable learnings found"; exit 0; }

# Write to memory, tagged with source + date
mkdir -p $LAZAR_HOME/memory/distilled
TS=$(basename "$ARCHIVE" | grep -oE '[0-9]+')
DATE=$(date -u +%Y-%m-%d)

# Split the output by section, append each to its category file
awk -v ts="$TS" -v date="$DATE" -v home="$LAZAR_HOME" '
  /^## / { cat = tolower(substr($0, 4)); next }
  /^- / && cat != "" {
    path = home "/memory/distilled/" cat ".md"
    print "- " substr($0, 3) " _(distilled " date ", from archive ." ts ".bak)_" >> path
    close(path)
  }
' /tmp/distill-out.md

echo "distilled into $LAZAR_HOME/memory/distilled/ (sections: $(ls $LAZAR_HOME/memory/distilled/ 2>/dev/null | tr '\n' ' '))"
rm -f /tmp/distill-out.md
```

## Output shape

After running, you'll have files like:

```
$LAZAR_HOME/memory/distilled/
├── gotchas.md
├── conventions.md
├── recipes.md
└── preferences.md
```

Each file contains tagged bullets:

```markdown
- When using bufio.Scanner on lazar's stream-json, set buffer to 10MB or it'll error on big tool_results _(distilled 2026-04-29, from archive .1745928000.bak)_
- The TUI lives at workspace/lazartui — never create parallel implementations _(distilled 2026-04-29, from archive .1745928000.bak)_
```

## Composing with other memory tiers

This skill creates a **richer L2** alongside `memory/log-summaries/<ts>.md` (the mechanical summaries from `log-rotation`):

- `log-summaries/*.md` — what *happened* (top prompts, top commands). Cheap, automatic.
- `distilled/*.md` — what was *learned* (gotchas, conventions, recipes). Costs LLM calls, opt-in.

`load-context` should read both — distilled insights first (high-signal), summaries second (broader but less curated).

## Decay

Distilled bullets carry `(distilled <date>, from archive ...)` tags. To prune stale entries:

```bash
# delete bullets older than 180 days
find $LAZAR_HOME/memory/distilled -name '*.md' -exec sed -i '' '/distilled 202[0-5]-/d' {} +
```

Or do this lazily: when reading a `distilled/` file at task start, the agent can filter on date and warn the user about stale entries. Don't over-engineer the lifecycle — markdown grep is fine.

## Anti-patterns

- Running `distill` on every prompt. Once per rotation max.
- Passing the whole archive into the prompt. Always sample.
- Distilling without tagging the source. Untraceable learnings rot silently.
- Mixing distilled bullets into top-level `memory/notes.md`. Keep them in `memory/distilled/` so the user can see what the LLM proposed vs. what they wrote themselves.
