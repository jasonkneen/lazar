---
name: harness
description: Continual harness — ranked scoped store for memories, skills, prompt notes, and subagent recipes. Auto-injected at session-start. Use to record durable lessons (add), load full entries, score helpful/unhelpful, and seed from playbook/learnings. Prefer this over freeform notes.md appends for anything that should affect future turns.
---

# Continual harness

Prime-agent-style persistent layer for lazar. **One store**, compact overview
always injected by `hooks/session-start.d/00-harness-overview.sh`.

## Paths

- Global state: `$LAZAR_HOME/user/memory/harness/state.json`
- History: `$LAZAR_HOME/user/memory/harness/history.jsonl`
- Local (session): `$LAZAR_HOME/user/memory/sessions/<id>/harness.json`
- CLI: `python3 $LAZAR_HOME/user/skills/_meta/harness/harness.py`

## Kinds

| Kind | Use for |
|------|---------|
| `memory` | Durable facts, prefs, pitfalls |
| `skill` | Pointers to procedures / when-to-use a skill |
| `prompt` | Narrow behavioral addenda (not full SOUL rewrites) |
| `subagent` | Delegation recipes for `lazar-spawn` |

## Scope

- `global` (default) — cross-session
- `local` — this `LAZAR_SESSION_ID` only

## Commands

```bash
# Compact overview (what session-start injects)
python3 $LAZAR_HOME/user/skills/_meta/harness/harness.py overview

# Record a lesson after success/failure
python3 $LAZAR_HOME/user/skills/_meta/harness/harness.py add \
  --kind memory \
  --title "short-slug" \
  --content "Specific actionable lesson" \
  --evidence "what happened"

# Load full entry; optional upvote
python3 $LAZAR_HOME/user/skills/_meta/harness/harness.py load M-never-cat-stream-log --helpful

python3 $LAZAR_HOME/user/skills/_meta/harness/harness.py helpful <id>
python3 $LAZAR_HOME/user/skills/_meta/harness/harness.py unhelpful <id>
python3 $LAZAR_HOME/user/skills/_meta/harness/harness.py delete <id>

# First-time / refresh seed (+ import playbook + learnings)
python3 $LAZAR_HOME/user/skills/_meta/harness/harness.py seed
python3 $LAZAR_HOME/user/skills/_meta/harness/harness.py stats
```

## Rules

1. **Smallest edit.** One concrete lesson beats a paragraph dump.
2. **Evidence.** Prefer `--evidence` when refining from a real failure/success.
3. **Global only if stable.** Session-only progress → `--scope local` (needs session id).
4. **Do not duplicate.** `add` skips near-duplicate content unless `--force`.
5. **Score usage.** When an injected bullet actually helped, `helpful <id>`.
6. Still write long provenance to `memory/distilled/` if needed — harness holds the *ranked* actionable slice.

## When to write

- User corrects you
- Same failure twice
- Stable preference discovered
- Spawn pattern that should be reused
- End of non-trivial task (1–2 bullets max)
