---
name: self-patch
description: How I notice a recurring failure of my own, root-cause it, fix what I can myself (behavioral rules, config, ignore files), and — for anything needing a kernel change I can't apply — draft a verified proposal for the operator to review. The capstone that turns my mistakes into durable fixes. Use when I hit a repeated mistake, get corrected by the operator, see a failed VERIFY, or notice a pattern worth fixing.
---

# self-patch — fixing myself

When I fail the same way twice, I fix it — I don't just apologize. This is the loop. It's
REACTIVE (invoked by the operator, or by me noticing mid-session) and TIERED: I fix the
non-kernel layers myself and verify; kernel changes I can only PROPOSE (my src/ is locked —
that's the safety boundary, not a limitation to route around).

## The loop: notice → root-cause → classify → act → record

### 1. Notice
A repeated mistake, operator feedback ("you did it again"), a failed VERIFY, or a pattern in
`memory/work-journal.md` / bounded logs.

### 2. Root-cause — read, don't guess
Read the actual code/logs. Confirm the mechanism before fixing. Use gitignore-aware search
(bare `rg` works via `.ignore`; `ls`/`find` for existence). Every good fix I've made came
from reading source, not assuming.

### 3. Classify the layer
- **behavioral** — a habit/instruction gap (I stall, I gaslight, I over-write). Fix = a rule
  in the PINNED hot set.
- **config** — an env var, rc file, or ignore file (rg blindness → `.ignore`; nudge count →
  `LAZAR_STALL_NUDGES`). Fix = change the file/var.
- **kernel** — needs Rust (max_tokens=8192, reset wipes memory/). I CANNOT edit the locked
  src/. Fix = draft a proposal (step 4c).

### 4. Act

**4a. Behavioral fix (autonomous).** Add ONE tight dated line to the PINNED hot set (see
below). Then PROVE it: end my turn with
`[VERIFY]\ncontains=memory/PINNED.md:<distinctive phrase from the rule>\n[/VERIFY]`.
Never claim a behavioral fix without a passing VERIFY.

**4b. Config fix (autonomous).** Change the file/var, then VERIFY the effect
(`contains=`, or a command whose output confirms it). Record it.

**4c. Kernel fix (PROPOSAL ONLY — never apply).**
  1. `staged=$(bash skills/_meta/self-patch/stage-proposal.sh <name>)` — stages a writable
     src/ copy under workspace/proposals/<name>/src/.
  2. Delegate the mechanical edit+build to a worker (isolates it from my context) by writing a
     PRECISE spawn prompt — exact file, exact old→new text, and an `EXPECT-VERIFY:` block
     requiring a build-ok marker — then:
     `bash "$LAZAR_HOME/bin/lazar-spawn" <name> <prompt-file>`.
     The worker edits `$staged`, runs
     `cd $staged && CARGO_TARGET_DIR=/tmp/sp-$$ cargo build --release` and, on success, writes
     `$staged/../build-ok` + a short `DESIGN.md`. (Or, for a tiny edit, I do this directly.)
  3. Gate on the envelope (`matched:true` = it compiled). NEVER run `kernel-apply.sh` myself.
  4. Surface to the operator, verbatim:
     "Kernel proposal `<name>` is staged and verified-building. Review the diff and run
     `bash scripts/kernel-apply.sh <name>` to apply."

### 5. Record
`bash skills/_meta/self-patch/ledger.sh <layer> <symptom> <cause> <fix> <verified>` →
appends to `memory/distilled/self-patch.md`. The lesson persists.

## PINNED hot-set discipline (keep the every-session injection lean)
- Behavioral rules live under `## Behavioral rules (hot)` in PINNED, **≤6 lines**, each dated.
- Promote a rule when a failure recurs; retire the coldest to
  `memory/distilled/behavioral-history.md` when the hot set overflows. Full history is never
  lost — only the always-injected copy is bounded.

## Boundaries (never cross)
- Never edit `src/` directly (locked). Never run `kernel-apply.sh` myself — kernel changes are
  proposal-only, operator-applied.
- Never touch `workspace/tui` (owner-managed).
- Never pattern-delete under `memory/distilled/`.
- Never call a fix done without a passing VERIFY.

## Self-test
`bash skills/_meta/self-patch/selftest.sh`
