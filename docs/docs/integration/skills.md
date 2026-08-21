# Skills

Skills are lazar's only extension mechanism. The kernel is immutable; all
capability growth happens by writing folders under `~/lazar/skills/`.

## Anatomy

A skill is a directory with a `SKILL.md`:

```text
skills/
├── INDEX.md                  one line per skill: name — description
├── <name>/
│   ├── SKILL.md              frontmatter (name, description) + instructions
│   └── ...                   scripts, templates, fixtures the skill needs
└── _meta/                    skills about being lazar (subagent, memory,
                              load-context, harness, self-patch, ...)
```

`INDEX.md` is the discovery surface — the agent reads it at the start of any
non-trivial task, then reads the relevant `SKILL.md` in full before acting.

## Writing one

1. `mkdir ~/lazar/skills/<name>`
2. Write `SKILL.md` with YAML frontmatter:

   ```markdown
   ---
   name: my-skill
   description: One sentence — when to load this skill.
   ---

   # my-skill

   Instructions, recipes, exact commands, failure modes.
   ```

3. Append one line to `skills/INDEX.md`.

Keep `SKILL.md` concrete: exact commands with absolute paths, bounded recipes
(`tail -n 200`, never `cat` an unbounded file), and the failure modes that
motivated the skill.

## The `_meta/` skills

`_meta/` holds the operating basics, and they outrank other skills on
conflict:

- **memory** — persistence across sessions (`memory/` notes, work journal).
- **subagent** — delegation via `bin/lazar-spawn` / `pack-and-spawn.sh`.
- **load-context** — bounded recipes for reading `logs/stream.jsonl`.
- **harness** — durable ranked lessons (`harness.py add`).
- **self-patch** — fix your own recurring failures; draft kernel proposals,
  never self-apply.

## Invoking a skill deliberately

A prompt that starts with `[skill:name] …` (what lazartui sends for `/name …`) or `/name …` (an ACP slash command) is resolved by the kernel before the model sees it: `skills/name/SKILL.md` is inlined into the user message, followed by the operator's request. The model gets the instructions, not a convention to decode and a file to fetch. An explicit `[skill:name]` for a skill that does not exist is an error returned to the client, never a prompt to the model. The kernel logs a `skill_inject` event with the name, path, and byte count.

## Rules

- Skills live in `skills/` — never patch `bin/` or `src/` to add behavior.
- A skill that shells out must respect the sandbox write zones when
  guardrails are on (`skills/`, `memory/`, `workspace/`, `/tmp`).
- If a capability is missing, write a skill — don't wait for a kernel change.
