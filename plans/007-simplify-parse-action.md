# Plan 007: Collapse the repeated warn-and-fail blocks in hooks.rs parse_action

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- src/src/hooks.rs`
> Plan 004 legitimately modified run_hook in this file; `parse_action` should
> be untouched. If `parse_action` differs from the excerpts, STOP.

## Status

- **Priority**: P3
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/004 (same file; land the risky one first)
- **Category**: tech-debt (DRY / KISS)
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

`parse_action` (`src/src/hooks.rs:489-633`) validates a hook's stdout JSON.
Each of its ~10 rejection branches hand-writes the same block: build a `msg`,
`eprintln!` a WARN line whose last fragment is a ternary on
`strict_pre_tool_hook(...)` ("failing closed" vs a lenient note), then
`return hook_failure_action(event, script_name, msg)`. Roughly 130 of the
function's 145 lines are this boilerplate. One helper halves the function and
makes adding the next validation branch a two-liner.

## Current state

Representative branch (`src/src/hooks.rs:516-526`):

```rust
Err(e) => {
    let msg = format!("emitted invalid JSON ({e})");
    eprintln!(
        "[lazar] WARN: hook {} ({}) {}; {}",
        script_name,
        event.as_str(),
        msg,
        if strict_pre_tool_hook(event, script_name) { "failing closed" } else { "treating as Continue" }
    );
    return hook_failure_action(event, script_name, msg);
}
```

The ~10 branches and their lenient notes:

| Location (2f10eb7) | msg | lenient note |
|---|---|---|
| 499-511 | non-UTF8 stdout | "treating as Continue" |
| 514-527 | invalid JSON | "treating as Continue" |
| 529-539 | JSON without string action | "treating as Continue" |
| 542-559 (veto on wrong event) | veto is pre-tool only | "ignoring" |
| 560-573 (transform.command wrong event) | pre-tool only | "ignoring" |
| 574-586 (transform.output wrong event) | post-tool only | "ignoring" |
| 587-595 (transform w/o payload) | without command/output | "treating as Continue" |
| 596-609 (inject wrong event) | only session-start/user-prompt | "ignoring" |
| 610-619 (inject w/o context) | without string context | "using empty context" |
| 622-631 (unknown action) | unknown action '{other}' | "treating as Continue" |

Note two format variants: the first three include `({})` with
`event.as_str()` after the script name; the later ones bake the event into
`msg` instead. Unifying on ONE format is acceptable — the WARN line is
operator-facing stderr, not a parsed protocol — but say so in the commit
message. Tests do not assert on the stderr text (check:
`grep -n "WARN" src/src/hooks.rs` inside `mod tests` → no matches at 2f10eb7).

Existing helpers already present: `strict_pre_tool_hook` (hooks.rs:218-225),
`hook_failure_action` (hooks.rs:227-235).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unlock source | `bash scripts/kernel-unlock.sh` | src/ writable |
| Tests | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` | all pass |
| Lint | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo clippy --release` | zero warnings |
| Rebuild + relock | `bash scripts/kernel-build.sh` | "Kernel rebuilt and locked." |

## Scope

**In scope**:
- `src/src/hooks.rs` — `parse_action` and one new private helper.

**Out of scope**:
- The RETURNED `HookAction` values for every input — behavior-identical is
  the bar; the extensive `parse_action_*` and `strict_*` tests enforce it.
- `run_hook`, `fire`, discovery caching.
- Any change to what counts as strict (`strict_pre_tool_hook`).

## Git workflow

- Branch: `refactor/007-parse-action` off `main` (after 004).
- Suggested commit message: `refactor(hooks): single reject helper in parse_action`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add the helper

Above `parse_action`:

```rust
/// Reject invalid hook output: warn the operator, then fail open
/// (Continue) or closed (Veto) depending on strict mode. `lenient_note`
/// describes the fail-open consequence ("ignoring", "treating as Continue").
fn reject(event: HookEvent, script_name: &str, msg: String, lenient_note: &str) -> HookAction {
    eprintln!(
        "[lazar] WARN: hook {} ({}) {}; {}",
        script_name,
        event.as_str(),
        msg,
        if strict_pre_tool_hook(event, script_name) { "failing closed" } else { lenient_note }
    );
    hook_failure_action(event, script_name, msg)
}
```

### Step 2: Convert all ~10 branches

Each becomes e.g.:

```rust
Err(e) => return reject(event, script_name, format!("emitted invalid JSON ({e})"), "treating as Continue"),
```

Preserve each branch's exact `msg` content and lenient note from the table.

**Verify**: `cargo test --release` → all pass, especially
`parse_action_invalid_json_falls_back_to_continue`,
`parse_action_unknown_action_falls_back_to_continue`,
`strict_unknown_action_vetoes_while_non_strict_continues`,
`strict_transform_without_payload_vetoes`,
`parse_action_veto_ignored_on_post_tool`.

### Step 3: Lint, size check, rebuild

**Verify**:
- `cargo clippy --release` → zero warnings.
- `parse_action` body ≤ ~70 lines (was ~145): count with
  `awk '/^fn parse_action/,/^}/' src/src/hooks.rs | wc -l` (approximate is fine).
- `bash scripts/kernel-build.sh` → success, binary relocked.

## Test plan

- No new tests needed — the existing 12 `parse_action`/strict tests pin the
  full input→HookAction mapping. All must pass unmodified. Do NOT edit any
  test to make it pass; a failing test means the refactor changed behavior.

## Done criteria

- [ ] `grep -c "failing closed" src/src/hooks.rs` → 1 (only in the helper)
- [ ] `cargo test --release` → all pass, zero test-file edits
- [ ] `cargo clippy --release` → zero warnings
- [ ] `bash scripts/kernel-build.sh` succeeds; binary relocked
- [ ] Only `src/src/hooks.rs` modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- Any existing test fails and the fix would require editing the test.
- `parse_action` at HEAD no longer matches the branch table (drift).

## Maintenance notes

- New validation branches should call `reject(...)` — reviewer should flag
  any future inline `eprintln!("[lazar] WARN: hook ...")` in this function.
- The WARN-format unification (if the executor unified the two variants) is
  cosmetic stderr-only; no consumer parses it.
