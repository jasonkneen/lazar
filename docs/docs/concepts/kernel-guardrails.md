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

1. **Stage.** Write a full proposed source tree under
   `workspace/proposals/<name>/src/`. The `new-proposal.sh` script copies a
   skeleton; the `_meta/propose-kernel-patch` skill guides the agent.
2. **Review.** `scripts/kernel-apply.sh <name>` produces a diff against the
   current kernel for the operator to read.
3. **Confirm** from an interactive operator terminal — the script prompts
   before touching anything.
4. **Apply + rebuild.** The script backs up the current source, applies with
   delete semantics, rebuilds, and re-locks the installed binary.
   `scripts/kernel-verify.sh <name>` runs an optional per-proposal
   `verify.sh` (via `LAZAR_PROPOSAL_DIR` / `LAZAR_PROPOSAL_NAME`).

## The `scripts/kernel-*.sh` family

| Script | Purpose |
|---|---|
| `kernel-unlock.sh` | Unseal the kernel source/binary for editing (pairs with `kernel-build.sh`) |
| `kernel-edit.sh` | One-shot: unlock, open `main.rs` in `$EDITOR`, rebuild, relock |
| `kernel-build.sh` | Rebuild the kernel and relock; on failure the source stays writable so you can fix and retry. Pass `LAZAR_GUARDRAILS=off` to keep the kernel writable across rebuilds |
| `kernel-apply.sh` | Diff, confirm, and apply a staged proposal from `workspace/proposals/<name>/`, then rebuild |
| `kernel-verify.sh` | Run patch-specific verification (`verify.sh`) for a staged proposal |

The agent's role stops at step 1. Applying is always an operator act.
