# Rust TUI review follow-up — 2026-09-04

Rust `tui/` remains the active TUI source. This follow-up preserves the existing
worktree and local archives. Changes are not committed or installed into the
live `bin/` and `skills/` trees. The earlier execution-environment blockers are
resolved: unrestricted runs now pass the full kernel suite, ACP integration
suite, and Rust TUI suite including the real active-tool lifecycle test.
This establishes local transport/process behavior with a mock provider; it is
not live-provider or visual sign-off.

## Source changes against the supplied review

| Review items | Change |
|---|---|
| 1, 2, 4–6, 23, 24 | Cancellation reaches subprocess waits and dispatch boundaries. ACP prompt/close/delete handlers spawn concurrent tasks; awaiting inside a handler had prevented cancellation delivery. Close blocks new prompts and drains the worker before deletion. EOF drains workers and fires session-end. Each kernel-owned command group also has a close-on-exec lifetime pipe and a guardian, so abrupt owner death and cancellation of nested kernels cannot leave their command groups running. TUI diagnostics preserve the running turn, access changes are guarded, and EOF/start failure leaves restart available. |
| 3, 9, 11, 12 | Timeout escalation checks the whole process group after its leader exits. A watchdog cleans groups after runner death. Detached jobs have independent supervisors, bounded execution, atomic final records, and fail on nonzero exit. Empty claims fail before launch. Session budget updates use short POSIX locks. |
| 7, 8, 13 | Adapter exit status is passed to scoring. Final invocation/message lifecycle and actual stop reasons determine completion. Exact-file expectations use real newlines. |
| 10 | Failover clears inherited session/model state, resets depth, and refuses retry after hook dispatch as well as tool events. |
| 14–19 | Session-start context is retained across prompts. Replay retains exit status, omits oversized input/result pairs, and explicitly rejects continuation when no valid history survives. Complete next requests have a 64,000-byte ceiling. ACP session project metadata is persisted; older sessions without metadata have an unknown cwd instead of inheriting today's project. |
| 20–22 | The outer invocation owns SSE error reporting. Saved model selection is reused only for the same provider. ACP publishes its effective sandbox mode to the TUI. |
| 25–28 | Setup creates the installation root before archiving, checks source-generation prerequisites, and installs helpers/contracts from the same installed source generation. Build mode validation precedes compilation and binary mutation. Newly seeded runtime skills/hooks regain owner write permissions. The setup test uses default guardrails. |
| 29–34 | See staged landing and verification below. Architecture docs narrow the typed-boundary contract and describe replay transformations and limits. Protocol completion remains distinct from verified outcomes; no prose/keyword completion detector was added. The review's 1,000-token manual-review threshold is not adopted as a kernel token policy. |

## Verified here

- Kernel: all 120 unit tests passed, including loopback provider transport,
  nested sandbox execution, process-owner lifetime, and failed-exec regressions.
- Rust TUI: all 52 tests passed with `--include-ignored` and the real kernel
  configured, including the normally gated full lifecycle test.
- Real ACP stdio: nine tests passed. They cover framing, cancel during a
  running hook, active deletion without log recreation, cleanup on EOF,
  abrupt owner death, nested-kernel cancellation, failed-tool replay on the
  wire, project cwd after restart, and explicit context-limit failure when
  an oversized last turn cannot be replayed safely.
  The abrupt-exit and nested-cancellation tests first reproduced surviving
  commands before the kernel process guardian was added.
- `python3 scripts/review-regressions.py`: 14 tests passed. Includes actual
  timeout descendant cleanup, runner SIGKILL cleanup, detached ownership,
  failed-adapter scoring through the runner, concurrent spawn budgets,
  failover environment/hook handling, installer preflight, and fresh-root
  seeding with protected source and writable runtime files. The installer
  fixture uses stub builders; it does not substitute for real build validation.
- Full isolated `scripts/setup-selftest.sh` passed with default guardrails,
  including Rust kernel/TUI release builds, runtime permissions, helper
  installation, guardrails toggling, and recovery selftests.
- Existing eval and subagent selftests: 16 assertions passed in each. Provider
  failover selftest passed. Kernel ceremony selftest passed 20 assertions.

The full TUI → ACP → active-tool test passed against a local mock SSE provider.
For cancellation, shutdown, and restart, it asserts that both the running shell
and its child process disappear, a subsequent tool call does not run, and
session-end hooks fire. It also checks test-credential inheritance through the
real transport. The test exercises the TUI transport handle rather than keyboard
input or terminal rendering:

```sh
cargo build --offline --manifest-path src/Cargo.toml
LAZAR_TEST_KERNEL="$PWD/src/target/debug/lazar" cargo test --offline \
  --manifest-path tui/Cargo.toml \
  real_tui_acp_active_tool_cancel_shutdown_and_restart -- --ignored
```

Live-provider and visual checks have not been run. Installed binaries and
runtime bundles have not been refreshed with these changes.

Latest unrestricted run logs: `/tmp/lazar-unrestricted-kernel.log` (120 unit
tests and nine ACP integration tests) and `/tmp/lazar-unrestricted-tui-all.log`
(52 TUI tests, none ignored). These supersede the earlier permission-denied
results. `git diff --check` also passes.

## Staged landing

Keep changes unstaged until the final diff is reviewed. Suggested independently
reviewable stages (some require hunk-level separation in shared files):

1. Incomplete-SSE rejection and its existing regression.
2. ACP/TUI cancellation, shutdown, warnings, and reconnect, with process tests.
3. Eval scoring and portable timeout ownership, then supervised spawn/failover.
4. Session replay, context bounds, metadata, and provider/access state.
5. Installer generation and guardrails changes with setup validation.

## Archive and recoverability inventory

Live inspection confirms `.archive/legacy-go-tui-2026-09-04/` contains the
original nested Go repository at `6b8a3e8d581e8df84d19eaafd408187a74b17c2d`.
Its 12 modified tracked files and seven untracked Go files match the inventory
in its `ARCHIVE.md`. Binaries, scripts, plist, screenshot, backup files, and
old TUI runtime skills are also present. The archive is ignored by the root
Git repository; it is a local preservation copy, not a remote backup.

`.archive/project-history-2026-09-04/` contains the completed plans,
`docs/superpowers/`, the superseded eval runner, and the old report. Root commit
`d347e3a` records their removal from active paths; those tracked originals can
be recovered from its parent.

No kernel proposal/backup directories were found in the present `.archive/`,
and the queried proposal/backup paths have no root Git history. This does not
establish recovery of previously deleted untracked proposal or backup history.
No further history cleanup was performed in this follow-up.
