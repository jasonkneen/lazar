# The VERIFY Contract — Kernel-Enforced Honesty

**lazar's most distinctive feature: the agent cannot lie about file state,
because the kernel — not the model — does the checking.**

Most agent harnesses trust the model when it says "done." lazar doesn't.
When the agent ends a turn claiming outcomes ("I created the file", "the
build passes"), the kernel re-checks every claim against the real
filesystem before accepting the response. This page describes how the
system works, why it lives in the kernel, and why it matters.

## Why this exists

The system was born from a real failure: an earlier session fabricated a
complete report — "I rotated the log, here is the table of new sizes" —
without running any tools at all. The operator only caught it by manually
running `ls -la`. Language models are fluent; fluency is not evidence.

The fix could not be a prompt instruction ("please don't fabricate") —
the model can ignore instructions, and a self-evolving agent could even
edit them away. So enforcement was placed in the one place the agent
cannot reach: the immutable Rust kernel.

## The two enforcement layers

Both layers live in `src/src/verifier.rs` (~1,400 lines, compiled into
the `lazar` binary) and run on every terminal response.

### Layer 1 — the `[VERIFY]` block

When the agent claims outcomes, it must end its turn with a machine-
checkable block:

```text
[VERIFY]
exists=/Users/jkneen/lazar/docs/mkdocs.yml
size_at_least=/Users/jkneen/lazar/docs/site/index.html:1000
contains=/Users/jkneen/lazar/docs/RESULT.md:DONE
[/VERIFY]
```

The kernel parses the block (`parse_verify_blocks_strict`) and re-runs
each claim itself (`verify_all`) with real filesystem operations — `stat`,
existence checks, substring scans (bounded to 8 MB reads). Supported
claims:

| Claim | Meaning |
|---|---|
| `exists=<path>` | file/dir exists |
| `not_exists=<path>` | file/dir absent |
| `size=<path>:<bytes>` | exact size |
| `size_at_least=<path>:<n>` | size ≥ n |
| `size_at_most=<path>:<n>` | size ≤ n |
| `contains=<path>:<substr>` | file contains substring (max 160 chars) |
| `mtime_within=<path>:<sec>` | modified within the last n seconds |

Rules enforced by the kernel, not by politeness:

- **Max 8 claims per block** — excess lines are rejected as invalid
  (models that dump 14 fragile `contains=` claims thrash the correction
  loop; the cap forces `exists=`/`size_at_least=` discipline).
- **Unknown claim keys fail the block** even if other claims pass.
- The closing marker must be exactly `[/VERIFY]` on its own line.

### Layer 2 — the pattern detector

Independent of the VERIFY block, `scan()` + `should_flag()` look for
fabrication *tells* in the response text: past-tense completion language
("I fixed…", "Total time: …"), markdown state tables, and checkmark
claims that are not backed by tool calls in the invocation. This layer
fires even with **no** VERIFY block present — one passing claim must not
globally suppress unsupported completion language elsewhere in the text.

## What happens on failure

When claims fail or unsupported completion language is detected:

1. The kernel **rejects the terminal response** and injects one bounded
   correction turn, telling the agent which claims failed. The agent must
   inspect and repair the real state (or retract the claim).
2. If the corrected response still fails, the invocation ends with a
   non-zero `grounding_failed` result — visible to callers, wrappers,
   and CI.
3. The retry budget is `LAZAR_GROUNDING_RETRIES` (default 2). It is
   bounded on purpose: grounding failures are loud, never silent.

Every verify report is also logged to `logs/stream.jsonl` as a
`verify_report` event, so the audit trail survives the session.

## Why the kernel, and not a skill?

| Concern | Where it lives | Why |
|---|---|---|
| Claim parsing & filesystem re-checks | `src/src/verifier.rs` (kernel) | The agent can read but never modify `bin/` or `src/` — it cannot patch away its own lie detector |
| Claim formats & when-to-claim guidance | system prompt + `verify` skill | Behavioral guidance the agent *can* evolve — but only toward writing claims that survive the Rust checks |
| Subagent claim gating | `bin/lazar-spawn` (locked binary) | Child tasks declare EXPECT-VERIFY claims authored by the parent; the spawn binary re-checks them before reporting success |

The split is deliberate: **enforcement is kernel-anchored, guidance is
evolvable.** A self-evolving agent gets smarter about *satisfying* the
contract; it never gets the option of *removing* it.

## What this buys you

- **Trustworthy "done."** When a lazar agent says a file exists with
  certain content, the kernel has already statted it.
- **Honest failure modes.** Agents that hit a wall say so — the
  alternative (a confident fabrication) is mechanically rejected.
- **Auditable sessions.** `verify_report` events in the event stream
  give you a post-hoc record of every claim and its outcome.
- **Composability.** CI pipelines and wrappers can treat a zero exit
  code as "the reported state was verified," not just "the model
  finished talking."

## See also

- [Architecture](architecture.md) — where the verifier sits in the kernel flow
- [Environment variables](../api/env-vars.md) — `LAZAR_GROUNDING_RETRIES`
- [Subagents](../integration/subagents.md) — EXPECT-VERIFY gating for child tasks
- [Skills](../integration/skills.md) — the `verify` skill: the agent-side discipline
