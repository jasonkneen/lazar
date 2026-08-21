---
name: guardrails
description: "Inspect lazar's enforcement layers and give the operator the exact command to turn them on or off. Use when a write fails with 'Operation not permitted', when the operator asks for full access, or when safety must be re-enabled. The operator-only script toggles sandbox-exec, kernel binary lock, src read-only mode, and pre-tool veto hooks."
version: 1.0.0
---

# guardrails

One switch for every enforcement layer in lazar.

## The four layers

| # | Layer | Mechanism |
|---|---|---|
| 1 | bash tool sandbox | `sandbox-exec` wrapper (`LAZAR_NO_SANDBOX`) |
| 2 | kernel binary lock | `chflags uchg bin/lazar` |
| 3 | source lock | `chmod a-w src/` |
| 4 | pre-tool vetoes | `hooks/pre-tool.d/*.sh` |

## Commands

```bash
bash $LAZAR_HOME/scripts/lazar-guardrails.sh status   # which mode am I in?
bash $LAZAR_HOME/scripts/lazar-guardrails.sh off      # full operator access
bash $LAZAR_HOME/scripts/lazar-guardrails.sh on       # restore enforcement
```

## When to use

- A write returned `Operation not permitted` and the operator wants it done.
- The operator asks to edit `bin/`, `src/`, `logs/`, or anything outside
  `skills/ memory/ workspace/`.
- The operator says "full access", "no sandbox", "stop blocking me".
- The operator asks to re-enable safety after risky work.

## Authority boundary

A sandboxed agent cannot disable its own sandbox: `.guardrails` is deliberately
outside its write allowlist. Run `status`, then tell the operator to execute the
exact `off` command in their terminal and start a fresh Lazar invocation. Do
not run `off` inside the current model-issued tool and claim it worked.

Wrong:
> "I can't write to logs/ — it's kernel-owned. Want me to use workspace/ instead?"

Correct:
```bash
bash $LAZAR_HOME/scripts/lazar-guardrails.sh status
# The operator runs this outside the current Lazar process:
bash $LAZAR_HOME/scripts/lazar-guardrails.sh off
```

If guardrails are already OFF, there is nothing to route around — just do the work.

## Important: mode changes need a fresh invocation

Layers 2–4 (locks, hooks) apply immediately. Layer 1 (sandbox) is decided when
the kernel process starts, so a *running* invocation stays in its current mode.
After toggling, the NEXT `lazar` call picks up the new mode. Tell the operator
that rather than pretending the current turn changed.

## Safety posture when OFF

Guardrails OFF removes *enforcement*, not *discipline*. Still true:

- Kernel edits go through `workspace/proposals/` + `scripts/kernel-apply.sh`.
- Back up before destructive edits (`engineering/safe-edit`).
- VERIFY still runs — grounding is independent of the sandbox.
- Do nothing the operator did not ask for. Full access is trust, not licence.

## Re-locking after a kernel rebuild

`scripts/kernel-build.sh` follows `$LAZAR_HOME/.guardrails`. To override that
choice for one build without changing the persisted mode:

```bash
LAZAR_GUARDRAILS=off bash $LAZAR_HOME/scripts/kernel-build.sh
```
