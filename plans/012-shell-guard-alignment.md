# Plan 012: Align shell guards — proposal-name validation, keychain-store exit codes, timeout resolution

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- scripts/new-proposal.sh src/seed-skills/_meta/env-vars/keychain-store.sh scripts/kernel-verify.sh workspace/evals/kernel-evals.sh`
> On any drift, compare the "Current state" excerpts before proceeding; on a
> mismatch, STOP.

## Status

- **Priority**: P3
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: tech-debt (consistency) + one small bug
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

Three small, independent alignment fixes, batched because each is a few
lines:

1. **Proposal-name validation is divergent.** The canonical guard
   `case "$NAME" in ""|.*|*/*|*..*|-*)` appears in `kernel-apply.sh:21`,
   `kernel-verify.sh:27`, and `self-patch/stage-proposal.sh:15`. But
   `new-proposal.sh:8-14` uses a different pattern
   (`*[!A-Za-z0-9._-]*|*/*|*..*|.*`) that does NOT reject a leading `-` —
   `new-proposal.sh -foo` creates a proposal named `-foo` that
   `kernel-apply.sh` will then refuse as unsafe. The guard is a
   path-traversal/option-injection defense on agent-supplied names; four
   copies with one looser copy is how guards rot.
2. **`keychain-store.sh` conflates skip and fail.** `store_one` returns 1
   both when a key is unset (skip) and when `security add-generic-password`
   fails (real failure). Consequence: in `--one` mode a storage FAILURE is
   tallied as SKIP and the script exits 0 — a silent failure. (The loop path
   papers over it by re-probing `printenv` twice.)
3. **timeout/gtimeout resolution disagrees across scripts.**
   `bin/lazar-spawn:90` prefers `timeout` over `gtimeout`;
   `scripts/kernel-verify.sh:55-60` prefers `gtimeout` over `timeout`;
   `workspace/evals/kernel-evals.sh:26` calls bare `timeout` unguarded
   (absent on stock macOS without coreutils).

Deliberately NOT a shared-lib extraction: these scripts must stay standalone
(ceremony scripts run before/without the skills tree; seed skills are
individually portable per README "Skills are portable"). Alignment, not
abstraction.

## Current state

- `scripts/new-proposal.sh:8-14`:

```bash
case "$NAME" in
    *[!A-Za-z0-9._-]*|*/*|*..*|.*)
        echo "invalid proposal name: '$NAME'" >&2
```

- Canonical (e.g. `scripts/kernel-apply.sh:20-22`):

```bash
case "$NAME" in
    ""|.*|*/*|*..*|-*) fail "unsafe proposal name: $NAME" ;;
esac
```

- `src/seed-skills/_meta/env-vars/keychain-store.sh:38-53` — `store_one`:
  `return 1` on both the "(not set)" skip branch and the "FAIL" branch.
  Call sites: `--one` at lines 58-59 (`[ $? -eq 1 ] && SKIP=... || FAIL=...`
  — the FAIL leg is unreachable since store_one never returns >1);
  loop at lines 61-71 re-probes `printenv "$name"` twice to reclassify.
- `bin/lazar-spawn:90`:
  `TIMEOUT_CMD="$(command -v timeout || command -v gtimeout || true)"`.
- `scripts/kernel-verify.sh:54-60`: prefers `gtimeout`, falls back to
  `timeout`, else runs unbounded.
- `workspace/evals/kernel-evals.sh:26`:
  `run(){ timeout "${EVAL_TIMEOUT:-120}" "$BIN" ... }` — unguarded.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Syntax | `bash -n <each touched file>` | exit 0 |
| Store dry test | see Step 2 verify | exit 1 on forced failure |
| Name-guard test | see Step 1 verify | `-foo` rejected |

## Scope

**In scope**:
- `scripts/new-proposal.sh` (pattern only)
- `src/seed-skills/_meta/env-vars/keychain-store.sh` (return codes + call sites)
- `scripts/kernel-verify.sh`, `workspace/evals/kernel-evals.sh`,
  `bin/lazar-spawn` (timeout precedence only — pick ONE order)

**Out of scope**:
- Extracting any shared library (see Why this matters).
- `kernel-apply.sh` / `stage-proposal.sh` guards — they are the canonical
  form already.
- Any keychain script other than keychain-store.sh.

## Git workflow

- Branch: `advisor/012-shell-guards` off `main`.
- Suggested commit message: `fix(scripts): align name guards, keychain-store exit codes, timeout resolution`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Tighten new-proposal.sh's guard

Replace the pattern at `new-proposal.sh:9` with the union of both checks
(charset AND the canonical structural guard):

```bash
case "$NAME" in
    ""|.*|*/*|*..*|-*|*[!A-Za-z0-9._-]*)
```

Keep the existing error message and exit code.

**Verify**:
`bash scripts/new-proposal.sh -foo; echo rc=$?` → prints the invalid-name
error, `rc=2`. Also `bash scripts/new-proposal.sh 'a/b'` → rejected. A VALID
name will fail later at the skeleton check on a machine without
`workspace/proposals/_skeleton` — that's fine; only the guard is under test.

### Step 2: Distinct return codes in keychain-store.sh

In `store_one`: `return 2` on the skip branch ("(not set)"), keep `return 0`
on success, `return 1` on the `kc_set` failure branch. Update call sites:

```bash
if [ -n "$ONE" ]; then
  store_one "$ONE"; rc=$?
  case "$rc" in
    0) OK=$((OK+1)) ;;
    2) SKIP=$((SKIP+1)) ;;
    *) FAIL=$((FAIL+1)) ;;
  esac
else
  for name in "${LAZAR_KEY_NAMES[@]}"; do
    store_one "$name"; rc=$?
    case "$rc" in
      0) OK=$((OK+1)) ;;
      2) SKIP=$((SKIP+1)) ;;
      *) FAIL=$((FAIL+1)) ;;
    esac
  done
fi
```

(Removes the double `printenv` re-probe entirely.)

**Verify** (no keychain mutation — use an unset var and a forced failure):
- Skip path: `bash src/seed-skills/_meta/env-vars/keychain-store.sh --one DEFINITELY_UNSET_VAR_XYZ; echo rc=$?`
  → output shows `skip`, summary `skipped=1 failed=0`, `rc=0`.
- Fail path: force `kc_set` to fail without touching the real keychain by
  running with a service name the keychain rejects is unreliable — instead
  temporarily stub: `security(){ return 1; }; export -f security` in a bash -c
  wrapper:
  `NAME=X bash -c 'security(){ return 1; }; export -f security; VALUE=y X=y bash src/seed-skills/_meta/env-vars/keychain-store.sh --one X'; echo rc=$?`
  — if the function-export stub doesn't propagate (it must: bash exports
  functions to child bash), verify instead by code inspection + the skip
  path, and note it. Expected on success: summary `failed=1`, `rc=1`.

### Step 3: One timeout precedence everywhere

Adopt lazar-spawn's form (native `timeout` first — on macOS with coreutils
both exist and behave identically; prefer the one more likely present on
Linux too):

- `scripts/kernel-verify.sh:54-60` → replace the if/elif ladder with
  `TIMEOUT_CMD="$(command -v timeout || command -v gtimeout || true)"` and
  keep the existing "run plain if empty" fallback (adjust the invocation to
  `$TIMEOUT_CMD "$VERIFY_TIMEOUT" bash "$VERIFY_SCRIPT"`).
- `workspace/evals/kernel-evals.sh:26` → guard the same way; if neither
  exists, run unbounded (matching kernel-verify's philosophy) rather than
  erroring.
- `bin/lazar-spawn` — already the canonical form; no change (mode is 555:
  don't touch it).

**Verify**: `bash -n scripts/kernel-verify.sh workspace/evals/kernel-evals.sh`
→ exit 0. `bash scripts/kernel-verify.sh nonexistent-proposal; echo rc=$?` →
"proposal not found" error, `rc=2` (guard path still works).

## Test plan

- The per-step verifies above are the tests (guard rejection, skip/fail
  tallies, syntax checks). No selftest harness exists for these scripts;
  don't create one in this plan.

## Done criteria

- [ ] `bash scripts/new-proposal.sh -foo` → rejected, rc=2
- [ ] `grep -n "printenv" src/seed-skills/_meta/env-vars/keychain-store.sh | wc -l` → 1 (only inside store_one)
- [ ] keychain-store skip path → rc=0 with `skipped=1`
- [ ] `grep -c "command -v timeout" scripts/kernel-verify.sh workspace/evals/kernel-evals.sh` → 1 each
- [ ] `bash -n` clean on every touched file
- [ ] `bin/lazar-spawn` NOT modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- The function-export stub in Step 2's fail-path verify mutates the real
  keychain (it must not — the stub intercepts `security`); if you cannot
  verify without touching the keychain, verify the skip path only and say so.
- Changing kernel-verify's timeout invocation alters its exit-code mapping
  (124 = timeout) — the `case "$rc"` at kernel-verify.sh:69-73 must keep
  matching 124.

## Maintenance notes

- The canonical name-guard now exists in four aligned copies (apply, verify,
  stage-proposal, new-proposal). If a fifth consumer appears, THAT is the
  threshold to revisit a shared helper — until then standalone-ness wins.
- Reviewer: keychain-store's stdout format (`store/skip/FAIL` lines and the
  `---- stored=… ----` summary) must be unchanged — the SKILL.md documents it.
