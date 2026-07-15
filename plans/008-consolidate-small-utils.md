# Plan 008: Consolidate small duplicated utilities across kernel modules

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- src/src/`
> Plans 002/004/005/006/007 legitimately reshaped main.rs and hooks.rs.
> Re-locate every item below by symbol name; skip any item that no longer
> exists, and STOP only if an excerpt's live content contradicts the
> description.

## Status

- **Priority**: P3
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/005
- **Category**: tech-debt (DRY, polish)
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

Several tiny utilities are implemented twice in different modules. None is
individually costly; together they are the "which copy do I fix?" tax. This
plan unifies them with zero behavior change. It is deliberately last in the
Rust sequence — pure polish on top of the structural work.

## Current state

Duplications, at `2f10eb7`:

1. **UTF-8-safe byte truncation, twice.**
   `safe_prefix(s, max_bytes) -> &str` (`src/src/main.rs:118-128`) and
   `head_bytes(s, cap) -> &str` (`src/src/verifier.rs:696-705`) are the same
   function (walk `end` back to a char boundary, slice). Both are used by
   their module's truncation formatters.

2. **Assistant text extraction, twice.**
   `verifier::extract_text(content: &Value) -> String`
   (`src/src/verifier.rs:63-81`) and
   `session::extract_text_from_assistant_content(Option<&Value>) -> Option<String>`
   (`src/src/session.rs:204-225`) both concatenate `"text"` blocks with `\n`,
   accepting a plain-string content as-is. The session variant differs only
   in Option-wrapping (None when content is missing vs "" from the verifier
   version).

3. **`emit_event` re-inlines `now_millis`.**
   `src/src/main.rs:700-707` computes the epoch-millis expression inline;
   `now_millis()` (`src/src/main.rs:104-109`) is the same code.

4. **Three near-identical size arms in `check_claim`.**
   `VerifyClaim::Size` / `SizeAtLeast` / `SizeAtMost`
   (`src/src/verifier.rs:530-586`) differ only in comparator and note text.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unlock source | `bash scripts/kernel-unlock.sh` | src/ writable |
| Tests | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` | all pass |
| Lint | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo clippy --release` | zero warnings |
| Rebuild + relock | `bash scripts/kernel-build.sh` | "Kernel rebuilt and locked." |

## Scope

**In scope**:
- `src/src/main.rs`, `src/src/verifier.rs`, `src/src/session.rs`.

**Out of scope**:
- `stall.rs` (its `last_sentence` char handling is distinct, not a duplicate).
- Any output/format change: truncation markers, notes in ClaimResult, event
  timestamps must be byte-identical.
- Creating a new module for these — piggyback on existing homes (below);
  a `util.rs` for four functions is over-engineering.

## Git workflow

- Branch: `refactor/008-small-utils` off `main` (after 005).
- Suggested commit message: `refactor(kernel): dedupe byte-truncation, text extraction, size claims`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: One truncation helper

Keep `verifier::head_bytes` as the canonical implementation; make it
`pub(crate)`. In main.rs, delete `safe_prefix` and point its three callers
(`truncate_tool_output`, `tool_input_preview` — grep for others) at
`verifier::head_bytes`. (Direction chosen because verifier already has
boundary tests: `head_bytes_backs_off_to_char_boundary`,
`short_never_splits_multibyte`.)

**Verify**: `cargo test --release` → all pass;
`grep -n "fn safe_prefix" src/src/` → empty.

### Step 2: One text extractor

Make `session::extract_text_from_assistant_content` delegate:

```rust
fn extract_text_from_assistant_content(content: Option<&Value>) -> Option<String> {
    content.map(crate::verifier::extract_text)
}
```

CHECK first that semantics match: verifier::extract_text returns "" for an
array with no text blocks; the session caller (session.rs:169-173) already
guards `!text.is_empty()`, so "" vs Some("") is equivalent there. Also
confirm verifier::extract_text handles the plain-string case (it does,
verifier.rs:64-66). If any behavioral daylight appears, STOP.

**Verify**: `cargo test --release` → all pass, especially
`push_message_merges_adjacent_roles_after_tool_stripping` and the two
truncate_to_byte_cap tests.

### Step 3: emit_event uses now_millis

Replace the inline epoch computation in `emit_event` with `now_millis()`;
drop the local `use std::io::Write;` if the module-level import already
covers it (main.rs imports `io::{BufRead, Read, Write}` at line 15).

**Verify**: `cargo clippy --release` → zero warnings.

### Step 4: Fold the size-claim arms

In `check_claim`, replace the three size arms with one helper:

```rust
fn check_size_claim(claim: &VerifyClaim, path: &Path, expected: u64,
                    ok: impl Fn(u64, u64) -> bool, describe: &str) -> ClaimResult
```

producing notes identical to today's:
`"actual size {len} bytes (expected {n})"` / `"(need >= {n})"` /
`"(need <= {n})"` and the same `"stat failed: {e}"` failure note. The three
match arms become one-liners passing `|a, b| a == b` etc.

**Verify**: `cargo test --release` → all pass, especially
`check_size_validates_file_length`.

### Step 5: Rebuild

`bash scripts/kernel-build.sh` → success, binary relocked.

## Test plan

- No new tests; the existing suites (verifier truncation/claim tests,
  session merge/truncate tests, tool-output truncation tests in main.rs)
  pin all four behaviors. Zero test-file edits allowed.

## Done criteria

- [ ] `grep -rn "fn safe_prefix" src/src/` → empty
- [ ] `grep -c "duration_since" src/src/main.rs` → 2 (now_millis + now_nanos only; emit_event's inline copy gone — adjust count if plan 005 moved now_nanos to log.rs: then 1)
- [ ] `cargo test --release` → all pass, zero test edits
- [ ] `cargo clippy --release` → zero warnings
- [ ] `bash scripts/kernel-build.sh` succeeds; binary relocked
- [ ] `plans/README.md` status row updated

## STOP conditions

- Any consumer-visible string (truncation marker, claim note) would change.
- The session/verifier extractors turn out to differ semantically beyond
  Option-wrapping (Step 2 check fails).
- A duplicate listed here was already removed by an earlier plan — skip that
  step (not a STOP), note it in the status row.

## Maintenance notes

- If a future module needs byte-safe truncation, use `verifier::head_bytes`
  (or move it to a shared module if a third caller appears — that's the
  threshold where `util.rs` earns its existence).
- Reviewer: this diff should contain deletions and delegations only — any
  new logic is a red flag.
