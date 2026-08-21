# Helper scripts

One-line descriptions of the scripts in `~/lazar/scripts/`, taken from each
script's own header comments. These are operator tools — they run in your
terminal, not inside the agent loop.

| Script | Purpose |
|---|---|
| `kernel-build.sh` | Rebuild the kernel and follow persisted guardrails mode. Run after editing `src/`; `LAZAR_GUARDRAILS=on|off` overrides locking for one build. |
| `kernel-edit.sh` | One-shot: unlock, open `main.rs` in `$EDITOR`, rebuild, relock. For quick kernel edits. |
| `kernel-verify.sh` | Run mandatory patch-specific verification for a staged or frozen proposal (exports `LAZAR_PROPOSAL_DIR` / `LAZAR_PROPOSAL_NAME`). |
| `kernel-apply.sh` | Freeze a staged proposal outside the writable workspace, show its review material, confirm, create a trusted backup, apply, rebuild, and verify. |
| `kernel-rollback.sh` | Interactively restore a trusted `.archive/kernel-backups/` source and rebuild, after preserving a pre-rollback safety copy. |
| `kernel-ceremony-selftest.sh` | Offline destructive-path test of apply, automatic recovery, mandatory verification, and manual rollback. |
| `run-with-timeout.py` | Trusted process-group timeout used to bound proposal verification on macOS without GNU `timeout`. |
| `kernel-unlock.sh` | Unlock the lazar kernel source/binary for editing. Pair with `kernel-build.sh`. |
| `lazar-guardrails.sh` | One switch for ALL enforcement layers: `off` (full operator access — no sandbox, unlocked kernel), `on` (restore enforcement), `status` (which mode am I in?). |
| `lazar-chat.sh` | Minimal interactive REPL for lazar. No dependencies beyond bash; pipes assistant text through `glow` for markdown rendering when available. |
| `lazar-spawn.sh` | The trusted sub-agent spawn surface — installed to `$LAZAR_HOME/bin/lazar-spawn` by `setup.sh` (chmod 555). The copy in `scripts/` is the source; agents call the locked binary. |
| `lazar-failover.sh` | Safe cold-prompt provider cascade — installed to `$LAZAR_HOME/bin/lazar-failover`; retries a failed provider only before any structured tool event. |
| `new-proposal.sh` | Stage a complete source proposal with review/checklist templates and a verification script that fails closed until replaced. |
| `migrate-from-clawd.sh` | Move an older install at `~/clawd/lazar` to the new default home `~/lazar`. Preserves `skills/`, `memory/`, `workspace/`, `logs/`; supports `--yes` for non-interactive migration. |

Related reading: [Kernel guardrails](../concepts/kernel-guardrails.md) for the
proposal ceremony and [Sandbox](../concepts/sandbox.md) for the guardrails
switch.
