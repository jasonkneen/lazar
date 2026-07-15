# Plan 005: Shrink main.rs — extract the system prompt to a template file and the log machinery to log.rs

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- src/src/main.rs`
> Plans 002 and 004 legitimately modified main.rs. Re-locate the two regions
> this plan owns (the `system` format! block in `run_agent`, and the log
> rotation/append family) by symbol name, not by the line numbers below, and
> compare against the excerpts; on a content mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW-MED (the prompt move has one sharp edge: `format!` brace
  escaping; the log move is mechanical relocation)
- **Depends on**: plans/004
- **Category**: tech-debt (file size / KISS)
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

`src/src/main.rs` is 2,551 lines at the planned-at commit — 5× the
next-largest file. Two self-contained regions account for ~530 lines:

1. The agent's ~170-line system prompt lives inside a `format!` string
   literal (`main.rs:1403-1570`). Prompt edits are undiffable, unreviewable,
   and every brace needs `{{`-escaping. The repo already has the right
   pattern: `sandbox.sb` is a standalone file compiled in via `include_str!`
   (`main.rs:51`).
2. The stream-log machinery — rotation, archive pruning, `append_stream`,
   symlink-safe open helpers — is a coherent subsystem (`main.rs:712-1073`)
   with its own tests, independent of the agent loop.

Extracting both leaves main.rs focused on CLI + agent loop (~1,300 lines
including its remaining tests) and makes the prompt a reviewable artifact.

## Current state

(Line numbers are from commit `2f10eb7`; re-locate by symbol after 002/004.)

- `main.rs:49-51` — the include pattern to copy:

```rust
static SEED_SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/seed-skills");
static SEED_HOOKS: Dir = include_dir!("$CARGO_MANIFEST_DIR/seed-hooks");
pub(crate) static SANDBOX_PROFILE: &str = include_str!("../sandbox.sb");
```

- `main.rs:1403-1570` — `let system = format!("You are lazar, ...", ...)`
  with named args: `home_disp`, `skills_disp`, `max_depth`, `depth`,
  `sandbox_status_line`. IMPORTANT: the literal contains `format!`-escaped
  double braces at one spot — `{{kind, ts_ms, content, command, ...}}`
  (line 1469) — which must become SINGLE braces in the plain-text template.
- `main.rs:712-1073` — the log family, in order: `unique_log_archive_path`,
  `lock_file_exclusive` (+ non-unix stub), `ensure_dir_not_symlink`,
  `open_append_no_follow`, `write_new_no_follow`, `RotationInfo`,
  `maybe_rotate_log`, `log_archive_timestamp`, `harvested_archive_names`,
  `archives_to_prune`, `log_archive_keep_limit`, `prune_log_archives`,
  `append_stream`. Plus the thread-local `IN_LOG_ROTATION_HOOK`
  (main.rs:62-64) used only by `append_stream`.
- Callers of `append_stream` outside main.rs: `hooks.rs:250, 366, 470`
  (imported at `hooks.rs:27` via `use crate::{append_stream, ...}`).
  `open_append_no_follow` is used by `append_stream` and by the tests;
  `session.rs` has its own separate append (leave it alone).
- Tests to move with the code (in `main.rs mod tests`):
  `append_and_write_helpers_reject_symlinks`,
  `unique_archive_path_does_not_reuse_existing_archive`,
  `archives_to_prune_keeps_newest_requested_count_across_bak_variants`,
  `archives_to_prune_keep_zero_disables_pruning`,
  `archives_to_prune_ignores_active_lock_and_sessions`,
  `archives_to_prune_skips_unharvested`, and the `TempTestDir` helper struct
  (main.rs:2502-2526). NOTE: `TempTestDir` may still be used by remaining
  main.rs tests — if so, duplicate it in both test mods (test-only code;
  duplication acceptable) or move it to the new module and reference it as
  `crate::log::tests` is NOT possible across cfg(test) mods — duplicating is
  the simple correct choice.
- Module conventions: `mod` declarations at `main.rs:25-28`, `pub(crate)`
  visibility, `//!` module doc header.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unlock source | `bash scripts/kernel-unlock.sh` | src/ writable |
| Tests | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` | all pass |
| Lint | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo clippy --release` | zero warnings |
| Rebuild + relock | `bash scripts/kernel-build.sh` | "Kernel rebuilt and locked." |
| Prompt parity check | see Step 3 | identical output |

## Scope

**In scope**:
- Create `src/system-prompt.txt` (sibling of `src/sandbox.sb`).
- Create `src/src/log.rs`.
- `src/src/main.rs` — remove both regions, add `mod log;`, load the prompt.
- `src/src/hooks.rs` — import path update only (`crate::log::append_stream`
  or keep a `pub(crate) use` re-export in main.rs; prefer updating the import).

**Out of scope**:
- Any wording change to the system prompt. Byte-identical output is the bar.
- `session.rs` append logic.
- The agent loop, verifier, stall modules.

## Git workflow

- Branch: `refactor/005-split-main` off `main` (after 004 is merged/landed).
- Two commits: one per extraction. Suggested messages:
  `refactor(kernel): system prompt as template file (include_str!)` and
  `refactor(kernel): move stream-log machinery to log.rs`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Unlock; extract the prompt text

`bash scripts/kernel-unlock.sh`. Create `src/system-prompt.txt` containing the
prompt exactly as it renders today, with these placeholder tokens where the
`format!` named args appear: `{home_disp}`, `{skills_disp}`, `{max_depth}`,
`{depth}`, `{sandbox_status_line}`.

Transcription rules (this is the sharp edge):
- Every `\n\` line-continuation in the Rust literal becomes a real newline.
- Every `\"` becomes `"`.
- The doubled braces `{{kind, ts_ms, content, command, ...}}` become single
  braces `{kind, ts_ms, content, command, ...}`.
- Keep the placeholder tokens EXACTLY as written above (they are also the
  format-arg names, which makes the diff reviewable).

### Step 2: Load the template in run_agent

Add next to `SANDBOX_PROFILE` (main.rs:51):

```rust
static SYSTEM_PROMPT_TEMPLATE: &str = include_str!("../system-prompt.txt");
```

Replace the `let system = format!( ... )` block with sequential `.replace()`
calls (no new dependencies — KISS):

```rust
let system = SYSTEM_PROMPT_TEMPLATE
    .replace("{home_disp}", &home.display().to_string())
    .replace("{skills_disp}", &skills.display().to_string())
    .replace("{max_depth}", &MAX_DEPTH.to_string())
    .replace("{depth}", &depth.to_string())
    .replace(
        "{sandbox_status_line}",
        if no_sandbox_active() {
            "Current mode: SANDBOX OFF (--no-sandbox is active)."
        } else {
            "Current mode: sandboxed (default)."
        },
    );
```

ORDER MATTERS: `{max_depth}` must be replaced BEFORE `{depth}` would be safe
anyway (distinct tokens), but keep this order so `{depth}` never matches
inside `{max_depth}` — actually `.replace("{depth}", ..)` WOULD also hit the
tail of `{max_depth}` if `{max_depth}` were still unreplaced; the order above
prevents that. Do not reorder.

### Step 3: Prove prompt parity

Add a temporary test (or use this one permanently) in `main.rs mod tests`:
`system_prompt_template_has_no_unreplaced_placeholders` — render the template
with dummy values and assert the result contains none of the five `{...}`
tokens and DOES contain sentinel phrases: `"You are lazar"`,
`"VERIFY CONTRACT"`, `"{kind, ts_ms"` → wait, after replacement the literal
`{kind, ts_ms, content, command, ...}` MUST still be present (it is prompt
content, not a placeholder). Assert exactly that:

```rust
assert!(rendered.contains("{kind, ts_ms, content, command, ...}"));
assert!(!rendered.contains("{home_disp}"));
```

**Verify**: `cargo test --release` → all pass including the new test.

### Step 4: Create log.rs and relocate the log family

Create `src/src/log.rs` with a `//!` header ("stream-log machinery: append,
rotation, pruning") and MOVE (verbatim) the items listed in Current state,
plus `IN_LOG_ROTATION_HOOK`, plus the six tests and a `TempTestDir` copy.
Adjust visibility: `append_stream` stays `pub(crate)`; internal helpers can
drop to private unless main.rs still calls them (check `write_new_no_follow`
— used only by `maybe_rotate_log`; `ensure_dir_not_symlink` — used only by
`maybe_rotate_log`; `open_append_no_follow` — used by `append_stream` and
the symlink test). `log.rs` will need
`use crate::{lazar_home, now_millis};` and the `hooks` import for
`hooks::fire(HookEvent::LogRotation, ...)`.

Add `mod log;` in main.rs; update main.rs call sites to `log::append_stream`
(or add `use log::append_stream;`) and update `hooks.rs:27` to
`use crate::log::append_stream;`.

Note: `now_nanos` (main.rs:111-116) is used only by
`unique_log_archive_path` — move it into log.rs.

**Verify**: `cargo test --release` → all pass (same count as before this
step; the six tests now run as `log::tests::*`).

### Step 5: Lint, size check, rebuild

**Verify**:
- `cargo clippy --release` → zero warnings.
- `wc -l src/src/main.rs` → ≤ 1900 (was 2551; ~170 prompt + ~360 log +
  ~120 log tests removed).
- `bash scripts/kernel-build.sh` → success, binary relocked.

### Step 6 (optional, operator-gated): live smoke

With an API key: `lazar -p "reply with the single word OK"` → prints OK;
no behavioral difference expected. Skip without a key.

## Test plan

- New: `system_prompt_template_has_no_unreplaced_placeholders` (Step 3).
- Moved: the six log tests run unchanged under `log::tests`.
- Full suite green before/after each step.

## Done criteria

- [ ] `src/system-prompt.txt` exists; `grep -c "You are lazar" src/system-prompt.txt` → 1
- [ ] `grep -n "You are lazar" src/src/main.rs` → no matches (prompt is out)
- [ ] `grep -n "fn append_stream" src/src/main.rs` → no matches; `grep -n "fn append_stream" src/src/log.rs` → 1
- [ ] `cargo test --release` → all pass (77+: 76 pre-existing incl. plan-002/004 additions, +1 new)
- [ ] `cargo clippy --release` → zero warnings
- [ ] `wc -l src/src/main.rs` ≤ 1900
- [ ] `bash scripts/kernel-build.sh` succeeds; binary relocked
- [ ] `plans/README.md` status row updated

## STOP conditions

- The rendered prompt differs from the original beyond the intended
  placeholder substitutions (spot-check the VERIFY CONTRACT section and the
  `{kind, ts_ms...}` line — brace-escaping mistakes land exactly there).
- A moved log function turns out to have a caller this plan didn't list
  (grep before moving: `git grep -n "<fn name>" src/`).
- Test count drops (a moved test silently stopped compiling).

## Maintenance notes

- The prompt is now editable without touching Rust — but it is still compiled
  in (`include_str!`), so a rebuild (`scripts/kernel-build.sh`) is required
  for prompt changes to take effect. Consider documenting that in README's
  Upgrade section (deferred — docs change, separate concern).
- Reviewer: diff `src/system-prompt.txt` against the old literal rendered
  side-by-side; the only differences should be escaping.
- Follow-up deferred: `run_bash` could move to spawn.rs entirely (plan 004
  established the module); left out to keep this diff mechanical.
