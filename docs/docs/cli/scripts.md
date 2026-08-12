# Helper scripts

One-line descriptions of the scripts in `~/lazar/scripts/`, taken from each
script's own header comments. These are operator tools — they run in your
terminal, not inside the agent loop.

| Script | Purpose |
|---|---|
| `kernel-build.sh` | Rebuild the kernel and relock. Run after editing `src/`. If the build fails, the source stays writable so you can fix and retry. Pass `LAZAR_GUARDRAILS=off` to keep the kernel writable across rebuilds. |
| `kernel-edit.sh` | One-shot: unlock, open `main.rs` in `$EDITOR`, rebuild, relock. For quick kernel edits. |
| `kernel-verify.sh` | Run patch-specific verification for a kernel proposal — looks for `workspace/proposals/<name>/verify.sh` and executes it (exports `LAZAR_PROPOSAL_DIR` / `LAZAR_PROPOSAL_NAME`). |
| `kernel-apply.sh` | Apply a staged kernel proposal from `workspace/proposals/<name>/`: diffs against the current kernel, prompts for confirmation, then backs up, applies with delete semantics, rebuilds, and re-locks. |
| `kernel-unlock.sh` | Unlock the lazar kernel source/binary for editing. Pair with `kernel-build.sh`. |
| `lazar-guardrails.sh` | One switch for ALL enforcement layers: `off` (full operator access — no sandbox, unlocked kernel), `on` (restore enforcement), `status` (which mode am I in?). |
| `lazar-chat.sh` | Minimal interactive REPL for lazar. No dependencies beyond bash; pipes assistant text through `glow` for markdown rendering when available. |
| `lazar-spawn.sh` | The trusted sub-agent spawn surface — installed to `$LAZAR_HOME/bin/lazar-spawn` by `setup.sh` (chmod 555). The copy in `scripts/` is the source; agents call the locked binary. |
| `new-proposal.sh` | `new-proposal.sh <name>` — copy the proposal skeleton to a new proposal dir under `workspace/proposals/`. |
| `migrate-from-clawd.sh` | Move an older install at `~/clawd/lazar` to the new default home `~/lazar`. Preserves `skills/`, `memory/`, `workspace/`, `logs/`; supports `--yes` for non-interactive migration. |

Related reading: [Kernel guardrails](../concepts/kernel-guardrails.md) for the
proposal ceremony and [Sandbox](../concepts/sandbox.md) for the guardrails
switch.
