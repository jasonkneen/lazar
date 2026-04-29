# propose-kernel-patch

When you identify a change that needs to happen in the immutable kernel (`bin/lazar` or `src/`), you cannot apply it yourself — sandbox-exec blocks writes there, and `chflags uchg` blocks them at the OS level too. **Instead, stage a complete proposal in workspace/ that the user can review and apply with one command.**

## When to use

- You found a bug in the kernel (e.g. error path doesn't emit structured output, missing flag, incorrect timeout).
- You need a new kernel feature (e.g. a new CLI flag, a new event type in the stream).
- You want to update a seed skill (those are baked into the binary at compile time).
- You can articulate the patch precisely *and* it's small enough to review.

Do NOT use this for things that should be skills. The kernel does ONE thing — runs bash + records events. Adding behavior to skills is always preferred.

## How to use

1. Pick a short kebab-case slug for the proposal (e.g. `http-error-emit`, `add-resume-flag`).

2. Create the proposal directory:

       mkdir -p $LAZAR_HOME/workspace/proposals/<slug>/src/src
       mkdir -p $LAZAR_HOME/workspace/proposals/<slug>/src/seed-skills

3. Copy the current kernel source as the starting point, excluding generated build artifacts:

       rsync -a --exclude target/ $LAZAR_HOME/src/ $LAZAR_HOME/workspace/proposals/<slug>/src/

   If `rsync` is unavailable, copy `src/` and remove `target/` from the proposal before diffing/applying.

4. Apply your changes inside `$LAZAR_HOME/workspace/proposals/<slug>/src/`:

       # edit src/src/main.rs, src/sandbox.sb, src/seed-skills/<...> as needed

5. Write a README explaining what changed and why:

       cat > $LAZAR_HOME/workspace/proposals/<slug>/README.md <<'EOF'
       # <slug>

       ## Summary
       <one paragraph: what the change does>

       ## Why
       <observed problem or new capability needed>

       ## Files changed
       - src/src/main.rs (lines ~X-Y): <what>
       - src/seed-skills/...: <what>

       ## Risk
       <what could break, what to test after applying>
       EOF

6. Tell the user:

       I've staged a kernel patch at $LAZAR_HOME/workspace/proposals/<slug>/.
       Review with: diff -u --recursive $LAZAR_HOME/src $LAZAR_HOME/workspace/proposals/<slug>/src
       Apply with:  bash $LAZAR_HOME/scripts/kernel-apply.sh <slug>

## Principles

- **Never partial.** Always stage a complete source tree, but exclude generated artifacts such as `target/`. `kernel-apply.sh` applies with delete semantics, so the proposal tree must contain every source file that should remain.
- **Always include a README.** A patch you can't explain isn't a patch worth applying.
- **Keep proposals small.** Big rewrites should be split. The user has to read every line of the diff.
- **Prefer skills over kernel changes.** If you can express the fix as a skill, do that instead — no upgrade ceremony required.
- **Record the proposal in memory** after the user applies it, so future-you knows what kernel features exist.

## Anti-pattern

Do not "propose" by just describing the change in prose. Stage the full edited src/ tree. The user shouldn't have to re-derive your patch from your description.
