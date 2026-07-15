# Plan 003: Make setup.sh actually install lazar-spawn and fix its stale references

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- setup.sh bin/lazar-spawn src/seed-skills/_meta/subagent/spawn.sh src/seed-skills/_meta/subagent/selftest.sh`
> On any drift, compare the "Current state" excerpts before proceeding; on a
> mismatch, STOP.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug (install drift)
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

`bin/lazar-spawn` is the ONLY sanctioned sub-agent spawn surface: the seeded
pre-tool hook (`src/seed-hooks/pre-tool.d/10-spawn-discipline.strict.sh`)
vetoes raw `lazar -p`, and the kernel's system prompt tells the agent to
delegate via `$LAZAR_HOME/bin/lazar-spawn`. But:

1. `setup.sh` never installs or locks it. Its header claims "Installed to
   $LAZAR_HOME/bin/lazar-spawn (chmod 555 + chflags uchg)" — no script does
   this. It works today only because `bin/lazar-spawn` is git-tracked and the
   recommended install is clone-to-`~/lazar`.
2. On a separate-checkout install (`setup.sh` copies `src/` and `scripts/` to
   `$LAZAR_HOME` but not `bin/lazar-spawn`), delegation is fully bricked:
   the hook vetoes `lazar -p` and there is no spawn binary.
3. `src/seed-skills/_meta/subagent/spawn.sh` falls back to
   `$LAZAR_HOME/scripts/lazar-spawn.sh`, a file that does not exist anywhere
   in the repo, and its comment says the binary is "installed from
   scripts/lazar-spawn.sh" — stale.

Design decision (keep it KISS): install with `chmod 555` but do NOT
`chflags uchg` the tracked file — `uchg` on a git-tracked file breaks
`git pull`/`git checkout` when it changes, and the sandbox profile already
denies writes to `bin/` from sandboxed agent bash (only `--no-sandbox` mode
could touch it, which is explicit operator opt-in). Update the header comment
to say what is actually true.

## Current state

- `setup.sh:66-75` — installs and locks `bin/lazar` (the compiled kernel)
  only. `grep -n "lazar-spawn" setup.sh scripts/*.sh` → no matches.
- `bin/lazar-spawn:2-5` — header:

```bash
# lazar-spawn — the ONLY trusted sub-agent spawn surface.
#
# Installed to $LAZAR_HOME/bin/lazar-spawn (chmod 555 + chflags uchg).
```

- `src/seed-skills/_meta/subagent/spawn.sh:8-14`:

```bash
# Prefer locked install; fall back to scripts copy under LAZAR_HOME (not under skills/).
for candidate in \
  "$LAZAR_HOME/bin/lazar-spawn" \
  "$LAZAR_HOME/scripts/lazar-spawn.sh"
do
```

- `src/seed-skills/_meta/subagent/selftest.sh:8-16` — carries TWO dead
  fallback branches for the same ghost path (`$LAZAR_HOME/scripts/lazar-spawn.sh`
  and the relative `$HERE/../../../../scripts/lazar-spawn.sh`), and line 66-67
  greps the ghost path when checking the `logs/subagent` default:

```bash
if [ -x "$LAZAR_HOME/bin/lazar-spawn" ]; then
  SPAWN="$LAZAR_HOME/bin/lazar-spawn"
elif [ -x "$LAZAR_HOME/scripts/lazar-spawn.sh" ]; then
  SPAWN="$LAZAR_HOME/scripts/lazar-spawn.sh"
elif [ -x "$HERE/../../../../scripts/lazar-spawn.sh" ]; then
  SPAWN="$(cd "$HERE/../../../../scripts" && pwd)/lazar-spawn.sh"
```

- `setup.sh:78-80` — copies `scripts/` into `$LAZAR_HOME/scripts/` when
  installing from a separate checkout:

```bash
if [[ "$SCRIPT_DIR" != "$LAZAR_HOME" ]]; then
    cp -R "$SCRIPT_DIR/scripts/." "$LAZAR_HOME/scripts/"
fi
```

- Shell conventions in `setup.sh`: bash, `set -euo pipefail`, `[[ ]]` tests,
  `fail()` helper, 4-space indent.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Syntax check | `bash -n setup.sh && bash -n src/seed-skills/_meta/subagent/spawn.sh` | exit 0 |
| Spawn lint self-check | `bash bin/lazar-spawn --lint-only t /dev/null 2>&1; echo $?` | prints a JSON error envelope + `1` (missing content is expected — proves it runs) |
| Subagent selftest | `bash src/seed-skills/_meta/subagent/selftest.sh` | exits 0 |

## Scope

**In scope**:
- `setup.sh` — add the lazar-spawn install block.
- `bin/lazar-spawn` — header comment only (lines 2-6). NO logic changes.
- `src/seed-skills/_meta/subagent/spawn.sh` — remove the dead fallback
  candidate and fix the comment.
- `src/seed-skills/_meta/subagent/selftest.sh` — remove the two dead
  fallback branches (lines 10-13) and the ghost-path grep leg (line 67).

**Out of scope**:
- `bin/lazar` handling in setup.sh (the compiled kernel install is correct).
- The spawn-discipline hook.
- Any behavioral change to lazar-spawn's lint/budget/verify logic.

## Git workflow

- Branch: `advisor/003-spawn-install` off `main`.
- Suggested commit message: `fix(setup): install lazar-spawn; drop dead spawn.sh fallback`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Install lazar-spawn in setup.sh

After the binary-install block (after line 75, `chmod -R a-w "$LAZAR_HOME/src"`),
add:

```bash
# Install the trusted sub-agent spawn wrapper alongside the kernel binary.
if [[ "$SCRIPT_DIR" != "$LAZAR_HOME" ]]; then
    cp "$SCRIPT_DIR/bin/lazar-spawn" "$LAZAR_HOME/bin/lazar-spawn"
fi
chmod 555 "$LAZAR_HOME/bin/lazar-spawn"
```

**Verify**: `bash -n setup.sh` → exit 0.

### Step 2: Fix the lazar-spawn header claim

Change `bin/lazar-spawn` line 4 from
`# Installed to $LAZAR_HOME/bin/lazar-spawn (chmod 555 + chflags uchg).` to
`# Installed to $LAZAR_HOME/bin/lazar-spawn by setup.sh (chmod 555; bin/ is not agent-writable under the sandbox).`

You may need `chmod u+w bin/lazar-spawn` first (it may be mode 555); restore
`chmod 555 bin/lazar-spawn` after editing.

**Verify**: `grep -n "chflags uchg" bin/lazar-spawn` → no matches.

### Step 3: Remove the dead fallback in spawn.sh

In `src/seed-skills/_meta/subagent/spawn.sh`, replace the candidate loop with
a direct check of the single real path, and fix the header comment
("installed from scripts/lazar-spawn.sh" → "installed by setup.sh"):

```bash
SPAWN="$LAZAR_HOME/bin/lazar-spawn"
if [ -x "$SPAWN" ]; then
  exec "$SPAWN" "$@"
fi
```

Keep the existing error JSON emission for the missing case.

**Verify**: `bash -n src/seed-skills/_meta/subagent/spawn.sh` → exit 0;
`grep -n "lazar-spawn.sh" src/seed-skills/_meta/subagent/spawn.sh` → no matches.

### Step 3b: Remove the ghost-path branches in selftest.sh

In `src/seed-skills/_meta/subagent/selftest.sh`, collapse the SPAWN
resolution (lines 8-16) to the single real path:

```bash
SPAWN="$LAZAR_HOME/bin/lazar-spawn"
if [ ! -x "$SPAWN" ]; then
  echo "[FAIL] no trusted lazar-spawn found"; exit 1
fi
```

And in Task 4 (lines 66-71), drop the second `rg` leg that greps
`$LAZAR_HOME/scripts/lazar-spawn.sh` — keep only the check against `$SPAWN`.

**Verify**: `bash -n src/seed-skills/_meta/subagent/selftest.sh` → exit 0;
`grep -c "lazar-spawn.sh" src/seed-skills/_meta/subagent/selftest.sh` → 0.

### Step 4: Run the subagent selftest

`bash src/seed-skills/_meta/subagent/selftest.sh`

**Verify**: exit 0.

Note: the runtime copy of spawn.sh lives at `skills/_meta/subagent/spawn.sh`
(gitignored, user-evolved). Do NOT edit it — seed skills under
`src/seed-skills/` are the source of truth; runtime skills belong to the
agent. If the selftest reads the runtime copy and fails for unrelated
reasons, report it, don't fix runtime state.

## Test plan

- `bash -n` on all three touched files.
- `bash src/seed-skills/_meta/subagent/selftest.sh` → exit 0.
- Manual gate (operator, optional): a fresh separate-checkout install into a
  temp `LAZAR_HOME` gets an executable `bin/lazar-spawn`.

## Done criteria

- [ ] `grep -c "lazar-spawn" setup.sh` → ≥ 2 (install block present)
- [ ] `grep -n "chflags uchg" bin/lazar-spawn` → empty
- [ ] `grep -rn "lazar-spawn.sh" src/seed-skills/` → empty
- [ ] `bash src/seed-skills/_meta/subagent/selftest.sh` → exit 0
- [ ] Only the four in-scope files modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- `setup.sh` already references lazar-spawn (drift — someone fixed it).
- The subagent selftest fails BEFORE your changes (pre-existing breakage —
  report, don't chase it).
- You find another installer that handles `bin/lazar-spawn` (search first:
  `git grep -ln "lazar-spawn"`).

## Maintenance notes

- If the operator later wants `chflags uchg` on lazar-spawn, it must be paired
  with a `chflags nouchg` step in any update path (git pull hook or setup.sh),
  mirroring how `kernel-build.sh` handles `bin/lazar`. Deferred deliberately.
- Reviewer: confirm spawn.sh still emits the JSON error envelope when the
  binary is missing (the fail-loud path is load-bearing for the agent).
