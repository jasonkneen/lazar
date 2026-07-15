# Plan 011: Bound the session-history load (stop full-parsing unbounded session logs)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- src/src/session.rs`
> On any drift in session.rs, compare the "Current state" excerpts before
> proceeding; on a mismatch, STOP.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: LOW-MED (must preserve the loaded-history invariants: starts
  user-role, text-only, under byte cap)
- **Depends on**: none (session.rs is untouched by plans 002–008)
- **Category**: perf
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

When `--session <id>` / `--resume` is used, `session::load_messages` reads and
JSON-parses EVERY line of `logs/sessions/<id>.jsonl`. That log mirrors
`tool_result` events (see `append_stream`'s mirror list in main.rs — kinds
`user | assistant | tool_result | invoke_start | invoke_end`), each up to
`LAZAR_TOOL_OUTPUT_MAX_BYTES` (default 200KB), and session logs are never
rotated. So a long-lived session file grows without bound, and every continued
turn parses megabytes of tool output that the loader then discards (only
user/assistant text survives, capped at 200KB). Separately,
`truncate_to_byte_cap` re-serializes the entire message array once per dropped
message — O(D × total) when trimming D messages.

Honest impact framing: this fires once per invocation (not per event) and only
for `--session`/`--resume` users; the operator's TUI mints a new session per
launch, which keeps most session files small. It's a growth cliff, not a
today-fire. Fix it while it's an S/M change.

## Current state

- `src/src/session.rs:137-181` — `load_messages`: opens the log, iterates
  `reader.lines()`, `serde_json::from_str` on every line (line 155);
  `tool_result` and other kinds fall to `_ => {}` (line 176) and are discarded
  after parsing. Then `truncate_to_byte_cap(messages)` (line 180).
- `src/src/session.rs:230-258` — `truncate_to_byte_cap`: while over cap,
  `messages.remove(0)` then recompute `serialized_size(&messages)` — a full
  `serde_json::to_string` of the whole array per iteration
  (`serialized_size`, lines 260-264).
- The cap: `DEFAULT_SESSION_HISTORY_MAX_BYTES = 200_000` (line 29), override
  via `LAZAR_SESSION_HISTORY_MAX_BYTES` (lines 231-235).
- Invariants documented at lines 133-136 and enforced today: loaded messages
  start user-role, contain only text content, total under the byte cap.
  Tests: `truncate_to_byte_cap_keeps_recent_messages`,
  `truncate_to_byte_cap_strips_leading_assistant_after_drop`,
  `push_message_merges_adjacent_roles_after_tool_stripping`
  (session.rs:347-396).
- `append_session` (lines 271-307) is the writer — NOT in scope.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unlock source | `bash scripts/kernel-unlock.sh` | src/ writable |
| Tests | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` | all pass |
| Lint | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo clippy --release` | zero warnings |
| Rebuild + relock | `bash scripts/kernel-build.sh` | "Kernel rebuilt and locked." |

## Scope

**In scope**:
- `src/src/session.rs` — `load_messages`, `truncate_to_byte_cap`, new tests.

**Out of scope**:
- `append_session` / the mirror list in main.rs — what gets WRITTEN to
  session logs stays the same (stream archaeology depends on it).
- Session log rotation (a separate design decision; see Maintenance notes).
- The loaded-history invariants — identical output for any input that fits
  the cap.

## Git workflow

- Branch: `advisor/011-bounded-session-load` off `main`.
- Suggested commit message: `perf(session): tail-bounded history load; linear truncation`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Read a bounded tail instead of the whole file

In `load_messages`, before line-iterating: stat the file; if its length
exceeds a read budget (use `4 × cap` where cap is the resolved
`LAZAR_SESSION_HISTORY_MAX_BYTES` — headroom because tool_result lines are
discarded), `Seek` to `len - budget`, read to end, and drop everything up to
and including the first `\n` (to discard the partial line). Parse only that
tail. Files under the budget behave exactly as today.

Keep the per-line `serde_json::from_str(...).ok()`-style tolerance — a
malformed line must skip, not abort (existing behavior via `match`).

### Step 2: Make truncation linear

Rewrite `truncate_to_byte_cap` to compute each message's serialized size ONCE
(`Vec<usize>` alongside), sum them, and pop indices from the front while the
running total exceeds the cap — no re-serialization per drop. Keep the
trailing "first message must be user-role" fixup loop exactly as-is
(lines 247-255).

Keep `serialized_size` for tests if they use it (they do, line 357).

### Step 3: Tests

Add to `session.rs mod tests`, modeled on the existing truncate tests:

1. `load_messages_tail_bound_skips_partial_line` — write a temp session log
   (use the `TempTestDir`-style temp pattern from main.rs tests, or plain
   `env::temp_dir()` + unique name as the existing session tests do) with a
   small `LAZAR_SESSION_HISTORY_MAX_BYTES`, many user/assistant events plus
   large `tool_result` filler lines; point `LAZAR_HOME` at the temp dir;
   assert the load returns only recent messages, first is user-role, and no
   message content contains the filler marker from the cut-off region.
   NOTE: setting `LAZAR_HOME`/env in tests — follow the save/restore pattern
   of `HookTestEnv` in hooks.rs:824-871 to avoid cross-test pollution.
2. `truncate_linear_matches_old_semantics` — same inputs as
   `truncate_to_byte_cap_keeps_recent_messages`, assert identical output
   (this can simply be strengthening the existing test with exact expected
   messages rather than the current `<=` bound).

**Verify**: `cargo test --release` → all pass (existing session tests
unmodified in their assertions except as noted, plus 2 new).

### Step 4: Lint, rebuild

`cargo clippy --release` → zero warnings; `bash scripts/kernel-build.sh`.

## Test plan

Covered in Step 3. The invariants under test: tail-bounded read discards the
partial first line; text-only + user-first + byte-cap semantics unchanged;
truncation output identical to the old algorithm on the existing fixtures.

## Done criteria

- [ ] `load_messages` stats + seeks; `grep -n "seek" src/src/session.rs` → ≥1 match
- [ ] `truncate_to_byte_cap` contains no `serialized_size(&messages)` call inside a loop
- [ ] `cargo test --release` → all pass, incl. 2 new tests
- [ ] `cargo clippy --release` → zero warnings
- [ ] `bash scripts/kernel-build.sh` succeeds; binary relocked
- [ ] Only `src/src/session.rs` modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- Any existing session test's ASSERTIONS must weaken to pass — that means the
  rewrite changed loaded-history semantics.

## Amendment (2026-07-15, after first execution attempt)

The original STOP condition on test flakiness fired: the process-global
`LAZAR_SESSION_HISTORY_MAX_BYTES` env var races between concurrently running
tests in the same binary (Rust's default multithreaded test runner), including
a LATENT race between the two pre-existing truncate tests. Save/restore
guards cannot fix concurrent races. Resolution (advisor decision): a
module-local `static ENV_LOCK: Mutex<()>` in `session.rs`'s test mod; every
env-mutating test (the two new ones AND the two pre-existing truncate tests)
takes the guard poison-tolerantly
(`ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())`) as its first line.
Editing the two pre-existing tests is authorized for the guard line only.
Stability bar: 15 consecutive clean runs of `cargo test --release session::`.
hooks.rs's similar latent pattern is explicitly out of scope.

## Amendment 2 (2026-07-15, after revision round 1)

ENV_LOCK fixed the intra-module race, but a control experiment exposed a
CROSS-module race on `LAZAR_HOME` between the new test's env flip and
hooks.rs's `HookTestEnv` (both mutate the process-global var under the
multithreaded test runner; ~3-4 failures per 10 full-suite runs, none at
baseline). Resolution (advisor decision): parameterize the loader —
`load_messages(id)` becomes a thin wrapper over a private
`load_messages_from_path(path)`, and the tail-bound test calls the inner
function directly with a temp path, never mutating `LAZAR_HOME` at all.
ENV_LOCK stays for the cap-var tests. Stability bar raised: 15/15 on
`session::` AND 10/10 consecutive full-suite runs. Recorded for posterity:
`std::env::set_var` under a threaded test runner is unsound by design
(unsafe in Rust 2024); avoid env-flipping tests in this crate — prefer
parameterized internals.

## Maintenance notes

- Session logs still grow unboundedly on disk; this plan bounds the READ
  cost only. If a long-lived-session workflow emerges, add rotation or
  write-side filtering of tool_result mirroring — design decision for the
  operator (changing what's mirrored changes what `--resume` archaeology can
  see; deliberately out of scope here).
- Reviewer: check the seek math (`len.saturating_sub(budget)`) and the
  partial-line drop; off-by-one there corrupts the first parsed line.
