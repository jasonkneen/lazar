# Memory

Memory is lazar's persistence layer: plain markdown under
`~/lazar/memory/`, readable by you and the operator with a bare `cat`. No
JSON blobs, no databases.

## Layers

| Layer | Path | Purpose |
|---|---|---|
| Pinned context | `memory/PINNED.md` | Read every session start: provider truth, code ownership, act-don't-narrate rules, search rules. |
| Working notes | `memory/*.md` | Timestamped notes, breadcrumbs for future invocations. |
| Work journal | `memory/work-journal.md` | Durable record of what was built (harvested by `_meta/log-harvest`). |
| Harness lessons | `memory/harness/` | Ranked durable lessons, managed by `_meta/harness/harness.py`. |
| Event stream | `logs/stream.jsonl` | Canonical append-only log (see [Sessions](sessions.md)) — recorded, not auto-loaded. |

## Basic usage

```bash
# Load at the start of a task
cat $LAZAR_HOME/memory/PINNED.md
cat $LAZAR_HOME/memory/*.md 2>/dev/null

# Append a timestamped note
printf '\n## %s\n\nThe thing worth remembering.\n' \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> $LAZAR_HOME/memory/notes.md
```

## The continual harness

Durable lessons go in the harness, not only freeform markdown:

```bash
python3 ~/lazar/skills/_meta/harness/harness.py add \
  --kind memory --title "my-lesson-slug" \
  --content "specific lesson" --evidence "what happened"
```

A compact ranked overview is injected at session-start by a hook. Load full
entries with `harness.py load <id>`; mark usefulness with
`helpful` / `unhelpful`.

## Discipline rules

- **Recall is always a command**, never a promise: `cat memory/PINNED.md`,
  bounded `tail` of the log, `rg <topic> memory/work-journal.md`.
- **Search works**: a `~/lazar/.ignore` file makes bare `rg` see
  `workspace/ memory/ skills/ logs/` while skipping multi-GB caches. Never
  add `-uu` (drags caches in). Never conclude a file "doesn't exist" from
  one search — confirm with `ls -la`.
- **Don't trust cached mental models**: before claiming "memory is current"
  or "journal is up to date", run `wc -c memory/work-journal.md` in the same
  turn. Numbers and the commands that produced them travel together.

See the `memory` skill (`skills/memory/SKILL.md`) for the full operating
basics.
