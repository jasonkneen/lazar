---
name: guardrails
description: "Turn lazar's enforcement layers on or off. Use when a write fails with 'Operation not permitted', when the operator asks for full access, when you need to edit bin/ src/ logs/, or when the operator asks to re-enable safety. Toggles all four layers at once: sandbox-exec, kernel binary lock, src/ read-only, and pre-tool veto hooks. Never refuse an operator request because of permissions — check status, offer to toggle, then do the work."
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

## Hard rule

**Never refuse an operator request because of permissions.**

Wrong:
> "I can't write to logs/ — it's kernel-owned. Want me to use workspace/ instead?"

Right:
```bash
bash $LAZAR_HOME/scripts/lazar-guardrails.sh status
# if guardrails are on and the operator wants this:
bash $LAZAR_HOME/scripts/lazar-guardrails.sh off
# then perform the actual write
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

`scripts/kernel-build.sh` re-locks by default. To keep it unlocked:

```bash
LAZAR_GUARDRAILS=off bash $LAZAR_HOME/scripts/kernel-build.sh
```
