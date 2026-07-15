# Plan 010: setup.sh delegates build/install/lock to kernel-build.sh

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- setup.sh scripts/kernel-build.sh`
> Plan 003 legitimately added a lazar-spawn block to setup.sh. If the
> build/install region (excerpted below) differs, STOP.

## Status

- **Priority**: P3
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/003 (both edit setup.sh; land 003 first)
- **Category**: tech-debt (DRY)
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

`setup.sh:59-75` duplicates the entire body of `scripts/kernel-build.sh`:
cargo build with the external target dir, atomic temp-binary swap, chmod 555,
chflags unlock/relock, and `chmod -R a-w src`. Two copies of the
install-and-lock sequence WILL drift (their error messages already differ).
The install path and the rebuild path should be the same code:
`kernel-build.sh` already contains its own preflight checks (cargo/chflags
present, LAZAR_HOME sane, src exists) and is safe to call from setup.

## Current state

- `setup.sh:59-75` (the duplicated region):

```bash
mkdir -p "$CARGO_TARGET_DIR"
echo "[lazar] cargo build --release (target: $CARGO_TARGET_DIR)"
( cd "$LAZAR_HOME/src" && CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo build --release )

BUILT="$CARGO_TARGET_DIR/release/lazar"
[[ -x "$BUILT" ]] || fail "built binary missing at $BUILT"

# Install atomically: prepare a locked temp binary, then swap it in.
echo "[lazar] installing binary atomically"
TMP_BIN="$LAZAR_HOME/bin/.lazar.$$.tmp"
rm -f "$TMP_BIN"
cp "$BUILT" "$TMP_BIN"
chmod 555 "$TMP_BIN"
chflags nouchg "$LAZAR_HOME/bin/lazar" 2>/dev/null || true
mv -f "$TMP_BIN" "$LAZAR_HOME/bin/lazar"
chflags uchg "$LAZAR_HOME/bin/lazar"
chmod -R a-w "$LAZAR_HOME/src"
```

- `scripts/kernel-build.sh` — same sequence with its own guards; honors
  `LAZAR_HOME` and `CARGO_TARGET_DIR` from env with identical defaults
  (`$LAZAR_HOME/workspace/.cargo-target`).
- ORDERING CONSTRAINT: setup.sh currently copies `scripts/` into
  `$LAZAR_HOME/scripts/` AFTER the build (lines 77-80). To delegate, the
  scripts copy must move BEFORE the build so
  `$LAZAR_HOME/scripts/kernel-build.sh` exists on separate-checkout installs.
  (When `SCRIPT_DIR == LAZAR_HOME` — the recommended clone-in-place install —
  it's already there.)
- setup.sh conventions: `set -euo pipefail`, `[[ ]]`, `fail()` helper,
  4-space indent.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Syntax | `bash -n setup.sh` | exit 0 |
| Full install run | `bash setup.sh` | ends with "[lazar] built and locked." |
| Kernel sanity | `bin/lazar --help` | usage text, exit 0 |
| Lock check | `ls -lO bin/lazar \| grep -q uchg && echo locked` | `locked` |

## Scope

**In scope**:
- `setup.sh` only.

**Out of scope**:
- `scripts/kernel-build.sh` — it is the canonical copy; do not modify it.
- The seeding, symlink, and final-banner sections of setup.sh.
- The LAZAR_HOME guard prologue in each script (triplicated across
  setup/build/unlock — accepted duplication: each script must be safely
  runnable standalone; do NOT extract a shared lib for three lines).

## Git workflow

- Branch: `advisor/010-setup-delegates` off `main` (after 003).
- Suggested commit message: `refactor(setup): delegate build+lock to kernel-build.sh`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Move the scripts install before the build

Relocate the block at setup.sh:77-80

```bash
if [[ "$SCRIPT_DIR" != "$LAZAR_HOME" ]]; then
    cp -R "$SCRIPT_DIR/scripts/." "$LAZAR_HOME/scripts/"
fi
```

to just before the build region (i.e., after the source-staging section that
ends at line 57).

### Step 2: Replace the duplicated region with delegation

Replace setup.sh:59-75 with:

```bash
CARGO_TARGET_DIR="$CARGO_TARGET_DIR" bash "$LAZAR_HOME/scripts/kernel-build.sh"
```

(`kernel-build.sh` reads `LAZAR_HOME` from env — already exported? CHECK:
setup.sh assigns but does not `export LAZAR_HOME`. Pass it explicitly:
`LAZAR_HOME="$LAZAR_HOME" CARGO_TARGET_DIR="$CARGO_TARGET_DIR" bash ...`.)

Keep everything after (lazar-spawn block from plan 003, seeding, `--help`
check, symlink, banner) unchanged. The banner's "rebuild:" instructions
(lines 121-124) remain valid.

**Verify**: `bash -n setup.sh` → exit 0.

### Step 3: Full install run

`bash setup.sh` on this machine (it is the live install; the script is
designed to be re-run — it preserves src/skills/memory/workspace and rebuilds
the binary).

**Verify**:
- Output contains `[kernel-build] cargo build --release` (proof of delegation)
  and ends with `[lazar] built and locked.`
- `bin/lazar --help` → exit 0.
- `ls -lO bin/lazar | grep -q uchg && echo locked` → `locked`.
- `find src -perm -u+w -type f | head -1` → empty (src re-locked).

## Test plan

- The full `bash setup.sh` run in Step 3 IS the test (idempotent by design).
- Kernel test suite as sanity: `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` → all pass.

## Done criteria

- [ ] `grep -c "cargo build" setup.sh` → 0 (build happens only in kernel-build.sh)
- [ ] `grep -c "chflags uchg" setup.sh` → 0 for bin/lazar (kernel-build owns it; plan 003's lazar-spawn block doesn't use uchg)
- [ ] `bash setup.sh` completes; binary locked; src locked
- [ ] Only `setup.sh` modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- `kernel-build.sh` fails when invoked from setup.sh in a way it doesn't when
  run directly (env difference) — report the exact difference, don't patch
  kernel-build.sh.
- The re-run of setup.sh touches skills/memory/workspace (it must not —
  abort and report; something else drifted).

## Maintenance notes

- setup.sh and kernel-build.sh now share one install path; any future change
  to locking/atomic-swap semantics lands in kernel-build.sh only.
- Reviewer: confirm the scripts-copy reordering doesn't break the
  clone-in-place path (`SCRIPT_DIR == LAZAR_HOME` skips the copy entirely).
