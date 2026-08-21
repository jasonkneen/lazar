# Kernel guardrails

The Rust kernel is the protected core. Runtime automation, hooks, skills, and
model-issued bash may **read** it, but they must not **write** to `src/` or
`bin/`. There is no Rust-side autopilot or self-evolver that patches or
rebuilds the kernel on its own — that is by design.

## Why the agent can't self-patch the kernel

Three independent mechanisms overlap:

1. **OS locks.** `bin/lazar` carries `chflags uchg` (OS-level immutable) and
   `src/` is `chmod -R a-w` after setup.
2. **Sandbox profile.** `src/sandbox.sb` grants writes only to `skills/`,
   `memory/`, `workspace/`, `/tmp`, and a few agent-CLI session dirs. `bin/`
   and `src/` are not in the allowlist.
3. **Prompt-level doctrine.** The system prompt instructs the agent that
   kernel changes are *proposal-only*: stage, never apply.

Even with enforcement layers off (see
[Sandbox — guardrails switch](sandbox.md#the-guardrails-switch)), the proposal
flow below remains the supported path.

## The proposal ceremony

Kernel changes follow a four-step, operator-reviewed flow:

1. **Stage.** `scripts/new-proposal.sh <name>` copies the complete live source
   (excluding generated `target/`) and creates README, checklist, and
   fail-closed verification templates under `workspace/proposals/<name>/`.
2. **Freeze + review.** `scripts/kernel-apply.sh <name>` atomically moves the
   proposal to `.archive/kernel-proposals/`, outside the agent-writable
   sandbox, then prints its README, verification script, and full source diff.
3. **Confirm** from an interactive operator terminal — the script prompts
   before touching anything.
4. **Apply + prove.** The script stores the current source under
   `.archive/kernel-backups/`, applies the frozen source with checksum/delete
   semantics, rebuilds with the checked-in lockfile, and runs mandatory
   proposal-specific `verify.sh`. Failure restores and rebuilds the trusted
   backup. The successful snapshot remains archived.

The apply command prints a backup ID. Use
`scripts/kernel-rollback.sh <backup-id>` for a later manual rollback. It shows
the reverse diff, requires interactive confirmation, saves the current source
as a pre-rollback safety copy, restores, and rebuilds.

## The `scripts/kernel-*.sh` family

| Script | Purpose |
|---|---|
| `kernel-unlock.sh` | Unseal the kernel source/binary for editing (pairs with `kernel-build.sh`) |
| `kernel-edit.sh` | One-shot: unlock, open `main.rs` in `$EDITOR`, rebuild, relock |
| `kernel-build.sh` | Rebuild the kernel and follow the persisted guardrails mode; `LAZAR_GUARDRAILS=on|off` overrides that mode for one build |
| `kernel-apply.sh` | Freeze, review, confirm, back up, apply, rebuild, and verify a staged proposal |
| `kernel-verify.sh` | Run mandatory patch-specific verification (`verify.sh`) |
| `kernel-rollback.sh` | Interactively restore a trusted backup and rebuild, preserving a pre-rollback safety copy |
| `kernel-ceremony-selftest.sh` | Exercise successful apply, automatic recovery, fail-closed verification, and manual rollback in an isolated fake install |

The agent's role stops at step 1. Applying is always an operator act.
