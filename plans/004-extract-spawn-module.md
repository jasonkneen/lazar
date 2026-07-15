# Plan 004: Extract shared subprocess scaffolding into src/src/spawn.rs

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- src/src/main.rs src/src/hooks.rs`
> On any drift, compare the "Current state" excerpts against the live code
> before proceeding; on a mismatch, STOP. (Plan 002 legitimately touches
> `parse_sse_reader` in main.rs — that region is NOT in this plan's scope and
> its drift is expected.)

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED — this touches the sandbox invocation path. The 76-test suite
  plus optional live evals gate it; behavior must be byte-identical.
- **Depends on**: plans/002 (land the small perf fix first so this large
  main.rs diff doesn't conflict with it)
- **Category**: tech-debt (DRY)
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

`run_bash` (main.rs) and `run_hook` (hooks.rs) each hand-build the same
subprocess scaffolding: the sandbox-exec argument list, `env_clear` plus the
base environment block, a 20-entry `PASSTHROUGH_VARS` const duplicated
VERBATIM in both files, the `setpgid` pre_exec, and a try_wait/timeout/
kill-process-group polling loop. That's ~150 duplicated lines whose divergence
is a security risk: if one copy of the sandbox setup is ever patched without
the other, tools and hooks run under different confinement. One module, two
callers.

## Current state

- `src/src/main.rs:453-694` — `run_bash(cmd: &str) -> String`. The duplicated
  regions inside it:
  - sandbox-exec command build: lines 484-507
  - `PASSTHROUGH_VARS` const: lines 563-577
  - base env block (`env_clear`, PATH, HOME, LAZAR_*, TERM, LANG): lines 518-543
  - `setpgid` pre_exec: lines 599-607
  - poll-wait/timeout/kill loop (50ms sleep): lines 624-650
- `src/src/hooks.rs:237-487` — `run_hook(script, payload_str, timeout, event)`.
  Its copies:
  - sandbox-exec command build: lines 265-287
  - base env block: lines 295-320
  - `PASSTHROUGH_VARS` const: lines 327-341
  - `setpgid` pre_exec: lines 349-358
  - poll-wait/timeout/kill loop (20ms sleep): lines 405-423
- Differences that must be PRESERVED (they are intentional, not drift):
  - run_bash executes `/bin/bash -c <cmd>`; run_hook executes `/bin/bash <script>`.
  - run_bash cwd: `$PWD` when unsandboxed else workspace; run_hook cwd: always workspace.
  - run_bash stdin: `Stdio::null()`; run_hook stdin: `Stdio::piped()` (payload).
  - run_bash extra env: `LAZAR_TOOL_ENV=1`, `LAZAR_DEPTH`, optional
    `LAZAR_SESSION_ID`, `LAZAR_MODEL`, optional ANTHROPIC key passthrough
    (lines 535-597); run_hook extra env: `LAZAR_HOOK_EVENT`,
    `LAZAR_HOOK_PAYLOAD_LEN` (lines 311-312).
  - Poll intervals differ (50ms vs 20ms) — keep each caller's interval.
- Existing module conventions: modules declared in `main.rs:25-28`
  (`mod hooks; mod session; mod stall; mod verifier;`), cross-module items are
  `pub(crate)`, module-level `//!` doc comment at top, tests co-located.
- `SANDBOX_PROFILE` is `pub(crate) static` in `main.rs:51`; `hooks.rs` already
  imports it via `use crate::{... SANDBOX_PROFILE};` (hooks.rs:27).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unlock source | `bash scripts/kernel-unlock.sh` | src/ writable |
| Tests | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` | all pass |
| Lint | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo clippy --release` | zero warnings |
| Rebuild + relock | `bash scripts/kernel-build.sh` | "Kernel rebuilt and locked." |
| Live smoke (optional, needs API key) | `bash workspace/evals/kernel-evals.sh` | `FAIL=0` |

## Scope

**In scope**:
- Create `src/src/spawn.rs`.
- `src/src/main.rs` — declare `mod spawn;`, rewrite `run_bash` internals to
  use it.
- `src/src/hooks.rs` — rewrite `run_hook` internals to use it.

**Out of scope**:
- `src/sandbox.sb` — the profile itself must not change.
- Any observable behavior: argument order to sandbox-exec, env var set,
  timeout semantics, kill ordering, output capping.
- `parse_action`, `fire`, discovery caching in hooks.rs.
- The SSE parser region of main.rs (plan 002 owns it).

## Git workflow

- Branch: `refactor/004-spawn-module` (repo history shows a
  `refactor/extract-modules` branch merge — match that convention).
- Commit per step. Suggested final message:
  `refactor(kernel): shared spawn scaffolding in spawn.rs (dedupe run_bash/run_hook)`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Unlock and create spawn.rs with the shared pieces

`bash scripts/kernel-unlock.sh`, then create `src/src/spawn.rs` containing:

```rust
//! Shared subprocess scaffolding for agent bash and lifecycle hooks.
//! Both run through the same sandbox profile — keep them identical by
//! construction: any confinement change happens HERE, once.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use std::{env, thread};

use crate::SANDBOX_PROFILE;

pub(crate) const PASSTHROUGH_VARS: &[&str] = &[ /* move the 20 entries verbatim from main.rs:563-577 */ ];

/// What /bin/bash should execute.
pub(crate) enum BashPayload<'a> {
    /// `bash -c <cmd>` — the agent tool path.
    Inline(&'a str),
    /// `bash <script>` — the hook path.
    Script(&'a Path),
}

/// Build the bash Command, wrapped in sandbox-exec unless `unsandboxed`.
/// Mirrors main.rs:484-507 / hooks.rs:265-287 exactly.
pub(crate) fn bash_command(payload: BashPayload, lazar: &str, unsandboxed: bool) -> Command { ... }

/// env_clear + the base env both callers share (PATH, HOME, LAZAR_HOME,
/// LAZAR_SKILLS/MEMORY/WORKSPACE/LOGS, TERM, LANG), plus LAZAR_NO_SANDBOX
/// and PASSTHROUGH_VARS when unsandboxed. Mirrors main.rs:518-583 minus
/// the tool-only vars, which stay at the call site.
pub(crate) fn apply_base_env(command: &mut Command, lazar: &str, bash_home: &str, unsandboxed: bool) { ... }

/// setpgid(0,0) pre_exec — mirrors main.rs:599-607.
pub(crate) fn isolate_process_group(command: &mut Command) { ... }

/// try_wait poll loop with timeout; kills the process group on expiry.
/// Returns (exit_status, timed_out). Mirrors main.rs:624-643 / hooks.rs:405-423.
pub(crate) fn wait_with_timeout(
    child: &mut std::process::Child,
    child_pid: u32,
    timeout: Duration,
    poll: Duration,
) -> (Option<std::process::ExitStatus>, bool) { ... }
```

Move the code bodies from the excerpted line ranges VERBATIM — this is a
relocation, not a rewrite. `bash_home` computation
(`env::var("HOME")... else lazar`) may also move into a small helper if it
keeps both call sites simpler.

Declare `mod spawn;` in `main.rs` next to the other mod declarations (line 25-28).

**Verify**: `cargo test --release` (module compiles; old code still in place,
so expect "unused" warnings at most from clippy — proceed).

### Step 2: Switch run_bash to the shared module

Replace main.rs lines 484-507 with `spawn::bash_command(...)`, lines 518-543
+ 556-583 with `spawn::apply_base_env(...)` followed by run_bash's OWN extras
(`LAZAR_TOOL_ENV`, `LAZAR_DEPTH`, session id, `LAZAR_MODEL`, ANTHROPIC key
block — keep at the call site, verbatim), the pre_exec block with
`spawn::isolate_process_group(&mut command)`, and the wait loop (624-643)
with `spawn::wait_with_timeout(&mut child, child_pid, limits.timeout,
Duration::from_millis(50))`. Keep the post-wait `kill_process_group` +
reader-join code untouched. Delete the now-unused `PASSTHROUGH_VARS` const
from main.rs.

**Verify**: `cargo test --release` → all pass. `cargo clippy --release` →
no dead-code warnings in main.rs.

### Step 3: Switch run_hook to the shared module

Same substitution in hooks.rs (its extras `LAZAR_HOOK_EVENT` /
`LAZAR_HOOK_PAYLOAD_LEN` stay at the call site; poll interval
`Duration::from_millis(20)`). Delete hooks.rs's `PASSTHROUGH_VARS` copy.
Remove `SANDBOX_PROFILE` from hooks.rs's `use crate::{...}` if no longer
referenced there.

**Verify**: `cargo test --release` → all pass, including the hook tests
`strict_hook_that_prints_transform_then_fails_vetoes` and
`strict_hook_that_prints_transform_then_times_out_vetoes` (these exercise the
real spawn path end-to-end with `LAZAR_NO_SANDBOX=1`).

### Step 4: Grep-audit for leftovers, lint, rebuild

**Verify**:
- `grep -c "PASSTHROUGH_VARS" src/src/*.rs` → exactly 2 matches total, both
  in `spawn.rs` (the const + doc) or spawn.rs + call-site references — the
  point: zero remaining array literals of the var names outside spawn.rs
  (`grep -n "SSH_AUTH_SOCK" src/src/main.rs src/src/hooks.rs` → empty).
- `grep -n "sandbox-exec" src/src/main.rs src/src/hooks.rs` → empty
  (only spawn.rs builds it).
- `cargo clippy --release` → zero warnings.
- `bash scripts/kernel-build.sh` → success, binary relocked.

### Step 5 (optional, operator-gated): live behavioral evals

If `ANTHROPIC_API_KEY` is available: `bash workspace/evals/kernel-evals.sh`
→ `FAIL=0`. If no key, skip and note it in the status row.

## Test plan

- The existing suite is the primary gate — hooks tests spawn real
  subprocesses through this scaffolding.
- Add one new test in `spawn.rs`'s own `mod tests`:
  `wait_with_timeout_kills_on_expiry` — spawn `/bin/bash -c "sleep 5"` with a
  50ms timeout and assert `timed_out == true` and the wait returns. Model
  after the process-handling style in `hooks::tests`.

## Done criteria

- [ ] `src/src/spawn.rs` exists with the four shared pieces + const
- [ ] `grep -n "SSH_AUTH_SOCK" src/src/main.rs src/src/hooks.rs` → empty
- [ ] `grep -n "sandbox-exec" src/src/main.rs src/src/hooks.rs` → empty
- [ ] `cargo test --release` → all pass (76 existing + 1 new)
- [ ] `cargo clippy --release` → zero warnings
- [ ] `bash scripts/kernel-build.sh` succeeds; binary relocked
- [ ] Only main.rs, hooks.rs, spawn.rs modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- The excerpted line ranges don't match the live code (drift beyond plan 002's
  SSE region).
- Any existing test fails after Step 2 or 3 and a single focused fix attempt
  doesn't restore it — the extraction changed behavior; report the diff.
- You find a third copy of the scaffolding somewhere else in src/ (report;
  the plan assumed two).
- Preserving the intentional differences (stdin mode, cwd rule, extra env)
  requires more than passing parameters — i.e., the abstraction is fighting
  you. Report rather than force a leaky signature.

## Maintenance notes

- After this lands, ANY change to sandbox invocation or the passthrough list
  happens in spawn.rs only. Reviewer should reject future PRs that re-add
  env/sandbox logic at the call sites.
- The `-D` parameter order passed to sandbox-exec is not semantically
  significant, but keep it stable anyway for diffability.
- Plan 005 (main.rs split) builds on this; land 004 first.
