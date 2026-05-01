# create-skill

When you need a capability that isn't in INDEX.md, author a new skill.

## When to use

- INDEX.md has nothing relevant.
- An existing skill could be adapted but doesn't fit the current need cleanly.
- You found yourself doing the same multi-step bash dance twice; capture it.

## How to use

1. Confirm the gap:

       cat $LAZAR_HOME/skills/INDEX.md

2. Pick a short, descriptive kebab-case name (e.g. `web-search`, `file-watch`, `git-status`).

3. Create the skill folder and SKILL.md:

       mkdir -p $LAZAR_HOME/skills/<name>
       cat > $LAZAR_HOME/skills/<name>/SKILL.md <<'EOF'
       # <name>

       <one-line purpose>

       ## When to use
       <triggers — what situation calls for this skill>

       ## How to use
       <ordered steps the next invocation will follow>

       ## Recipe
       <copy-pasteable bash; assume the agent runs it verbatim>
       EOF

4. Register it in the index:

       echo "- <name> — <one-line purpose>" >> $LAZAR_HOME/skills/INDEX.md

5. Verify:

       cat $LAZAR_HOME/skills/INDEX.md
       cat $LAZAR_HOME/skills/<name>/SKILL.md

## Principles

- Skills should be small. If a skill does two things, split it.
- Prefer composition: existing skills + bash + `lazar -p` recursion.
- Bash-first. If the recipe can't be expressed in bash, the skill is too big.
- Document the failure modes you hit while designing it. Future-you will thank you.
- Skills go in `$LAZAR_HOME/skills/`. Never write outside that tree to extend your capabilities.
