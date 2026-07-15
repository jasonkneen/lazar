# Plan 001: Remove dead tracked files (kernel-apply backup + stale APPLY-ORDER.md)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- scripts/kernel-apply.sh.bak-before-verify APPLY-ORDER.md`
> If either file changed since this plan was written, compare against the
> "Current state" facts below before proceeding; on a mismatch, STOP.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: tech-debt (dead code)
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

Two files ship to everyone who clones this repo but serve no purpose:

1. `scripts/kernel-apply.sh.bak-before-verify` (80 lines) is a stale backup
   copy of `scripts/kernel-apply.sh` taken before the verify step was added.
   The live script supersedes it entirely.
2. `APPLY-ORDER.md` (253 lines) is a one-off session checklist ("Apply order —
   full session work") describing kernel changes that are already merged
   (the `--no-sandbox` flag, session module, hooks module all exist in
   `src/src/` at HEAD). It references machine-specific paths like
   `~/lazar/workspace/proposals/no-sandbox-flag` that don't exist in a fresh
   clone.

For a project whose README opens with "The smallest self-evolving agent
harness", 333 lines of confusing cruft in the tracked tree is a real cost.

## Current state

- `scripts/kernel-apply.sh.bak-before-verify` — identical to
  `scripts/kernel-apply.sh` except it lacks the `kernel-verify.sh` rollback
  block (live script lines 78–92). Nothing references it:
  the string `bak-before-verify` appears nowhere else in tracked files.
- `APPLY-ORDER.md` — starts with `# Apply order — full session work`.
  Referenced nowhere in tracked files (no link from README.md or any script).

Confirm both facts yourself in Step 1 before deleting.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Reference check | `git grep -n "bak-before-verify"` | only the file's own path (or nothing) |
| Reference check | `git grep -ln "APPLY-ORDER"` | only `APPLY-ORDER.md` itself (or nothing) |
| Kernel tests (sanity) | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` | `76 passed; 0 failed` |

## Scope

**In scope** (the only changes allowed):
- Delete `scripts/kernel-apply.sh.bak-before-verify`
- Delete `APPLY-ORDER.md`

**Out of scope** (do NOT touch):
- `scripts/kernel-apply.sh` — the live script stays exactly as is.
- Anything under `workspace/`, `memory/`, `skills/`, `hooks/`, `logs/` —
  gitignored runtime state, not source.

## Git workflow

- Branch: `advisor/001-delete-dead-files` off `main`.
- One commit, message style matches repo history (`git log --oneline` shows
  lowercase imperative, e.g. "Remove legacy wrapper and env scripts"):
  suggested message: `Remove stale kernel-apply backup and APPLY-ORDER.md`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Confirm nothing references the two files

Run both reference checks from the commands table.

**Verify**: `git grep -ln "bak-before-verify"; git grep -ln "APPLY-ORDER"` →
each returns at most the file's own path. If any OTHER file references them,
STOP and report.

### Step 2: Delete via git

```bash
git rm scripts/kernel-apply.sh.bak-before-verify APPLY-ORDER.md
```

**Verify**: `git status --short` → shows exactly two `D ` lines, nothing else.

### Step 3: Commit

**Verify**: `git ls-files | grep -E "bak-before-verify|APPLY-ORDER"` → no output.

## Test plan

No code paths change; the kernel test suite is a sanity gate only.
Run: `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` → 76 passed.

## Done criteria

- [ ] `git ls-files | grep -E "bak-before-verify|APPLY-ORDER"` → empty
- [ ] `git status --short` → clean (after commit)
- [ ] Kernel tests still pass (76/76)
- [ ] `plans/README.md` status row updated

## STOP conditions

- Any tracked file other than the two targets references either filename.
- `git rm` refuses because a file has local modifications.

## Maintenance notes

- If the operator wants a durable record of the proposal ceremony, the
  README's "Kernel guardrails" section already documents it; APPLY-ORDER.md
  added nothing beyond a specific past session's steps.
- Reviewer: confirm the diff is deletions only.
