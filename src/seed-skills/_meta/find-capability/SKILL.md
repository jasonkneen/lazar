# find-capability

Decide whether a capability already exists, can be composed from existing skills, or needs a new skill.

## When to use

Whenever a task requires something beyond bare bash one-liners.

## How to use

1. Read the index:

       cat $LAZAR_HOME/skills/INDEX.md

2. If a single skill matches, read it and use it:

       cat $LAZAR_HOME/skills/<name>/SKILL.md

   Done.

3. If multiple existing skills could compose into the answer:

   - Sketch the chain (skill A produces X, skill B consumes X, ...).
   - Run it.
   - If it works AND the chain is non-trivial AND you'd reuse it, document the composition itself as a new skill (invoke create-skill).

4. If no skill or composition fits:

   Read `_meta/create-skill/SKILL.md` and author a new skill.

## Principle

Reach for existing skills before authoring new ones. The skill library is
the agent's memory of "how to do things"; growing it carelessly bloats the
index without adding capability.

A useful test: would a human reading INDEX.md a week from now find this
skill name self-explanatory and distinct from the others? If not, rename or
merge before committing.
