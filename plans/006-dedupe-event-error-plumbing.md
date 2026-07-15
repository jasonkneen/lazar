# Plan 006: Collapse run_agent's repeated error-exit and dual-emission boilerplate

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- src/src/main.rs`
> Plans 002/004/005 legitimately rewrote parts of main.rs. Re-locate the
> patterns below by CONTENT (grep for `session_guard.mark_error`), not line
> number. If the error-exit pattern no longer appears ≥ 6 times, STOP —
> someone already did this work.

## Status

- **Priority**: P3
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/005
- **Category**: tech-debt (DRY)
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

Every abort path in `run_agent` hand-writes the same four-step ritual, seven
times at the planned-at commit: `emit_stream_error(format, &msg)` →
`append_stream(json!({"kind": "invoke_end", ...}))` →
`session_guard.mark_error(&msg)` → `return Err(msg.into())`. Separately,
notable events are built twice — once for the canonical log
(`append_stream`, key `"kind"`) and once for stream-json consumers
(`emit_event`, key `"type"`) — with near-identical payloads at four sites.
A missed leg on a future error path means the log and the TUI silently
disagree about how an invocation ended. Two small helpers make the invariant
structural.

## Current state

(At `2f10eb7`; re-locate by grep.) The seven error-exit sites in `run_agent`,
`src/src/main.rs`:

- 1694-1699 (`LAZAR_MAX_TURNS` exceeded — stop_reason "aborted")
- 1720-1727 (API request failed — stop_reason "error")
- 1729-1743 (non-2xx API status — stop_reason "error"; also has a
  `verbose` eprintln leg)
- 1746-1754 (SSE parse error — stop_reason "error")
- 1986-1996 (`LAZAR_MAX_TOOL_CALLS` exceeded — stop_reason "aborted")
- 2100-2114 (consecutive empty turns — stop_reason "aborted"; also emits a
  stream-json `error` event and an eprintln)
- 2119-2127 (no executable calls — stop_reason "error")

Canonical example (1720-1727):

```rust
Err(e) => {
    let msg = format!("API request failed: {e}");
    emit_stream_error(format, &msg);
    append_stream(json!({"kind": "invoke_end", "stop_reason": "error", "note": &msg}));
    session_guard.mark_error(&msg);
    return Err(msg.into());
}
```

The dual-emission sites (same payload logged then emitted):
- auto_nudge: 1837-1848
- verify_report: 1887-1896
- fabrication_check: 1917-1932

(`invoke_end`/`tool_result` sites have differing payload shapes between the
two channels — leave them as-is.)

Helper context: `emit_stream_error` (main.rs:245-249) already gates on
`OutputFormat::StreamJson`. `SessionEndGuard` (main.rs:1269-1316) holds
`mark_error`. After plan 005, `append_stream` lives in `crate::log`.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unlock source | `bash scripts/kernel-unlock.sh` | src/ writable |
| Tests | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` | all pass |
| Lint | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo clippy --release` | zero warnings |
| Rebuild + relock | `bash scripts/kernel-build.sh` | "Kernel rebuilt and locked." |

## Scope

**In scope**:
- `src/src/main.rs` — two new private helpers + the listed call sites.

**Out of scope**:
- Event payload contents, key names, or ordering — the JSONL written to
  `logs/stream.jsonl` and the stream-json events must be byte-identical
  (consumers like the operator's TUI parse them).
- The success-path invoke_end handling.
- hooks.rs / log.rs / any other module.

## Git workflow

- Branch: `refactor/006-event-plumbing` off `main` (after 005).
- Suggested commit message: `refactor(kernel): abort_invoke + log_and_emit helpers (dedupe run_agent plumbing)`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add the abort helper

Near `run_agent`, add:

```rust
/// Uniform abort: mirror the error to stream-json consumers, record
/// invoke_end in the canonical log, mark the session guard, and build the
/// Err. Every early-exit in the agent loop MUST go through this.
fn abort_invoke(
    format: OutputFormat,
    session_guard: &SessionEndGuard,
    stop_reason: &str,
    msg: String,
) -> Box<dyn std::error::Error> {
    emit_stream_error(format, &msg);
    append_stream(json!({"kind": "invoke_end", "stop_reason": stop_reason, "note": &msg}));
    session_guard.mark_error(&msg);
    msg.into()
}
```

Convert the seven sites to `return Err(abort_invoke(format, &session_guard,
"error" /* or "aborted" */, msg));`. Keep each site's EXTRA legs (the
`verbose` eprintln at the non-2xx site, the eprintln + stream-json `error`
event at the empty-turns site) at the call site, in their current order
relative to the ritual.

**Verify**: `cargo test --release` → all pass;
`grep -c "session_guard.mark_error" src/src/main.rs` → 1 (only inside the
helper).

### Step 2: Add the dual-emission helper

```rust
/// Record an event in the canonical log (key "kind") and, in stream-json
/// mode, emit the same payload to stdout (key "type").
fn log_and_emit(format: OutputFormat, name: &str, payload: Value) {
    let mut logged = payload.clone();
    if let Some(obj) = logged.as_object_mut() {
        obj.insert("kind".into(), json!(name));
    }
    append_stream(logged);
    if format == OutputFormat::StreamJson {
        let mut emitted = payload;
        if let Some(obj) = emitted.as_object_mut() {
            obj.insert("type".into(), json!(name));
        }
        emit_event(emitted);
    }
}
```

Convert ONLY the three listed sites (auto_nudge, verify_report,
fabrication_check). Before converting each, diff the two payloads at that
site key-by-key; if they differ by more than the `kind`/`type` key
(auto_nudge does: the logged event has `nudge`/`max` fields the emitted one
lacks — check!), then either pass the superset and accept the extra fields on
the emitted event ONLY if consumers tolerate unknown fields — they do not
necessarily; instead SKIP any site whose payloads are not identical and note
it in the commit message. At `2f10eb7`, verify_report and fabrication_check
payloads are identical; auto_nudge differs (logged has `nudge`, `max`) —
expected outcome: convert 2 sites, leave auto_nudge.

**Verify**: `cargo test --release` → all pass; `cargo clippy --release` →
zero warnings.

### Step 3: Event-shape parity check, rebuild

Run a no-network parity check: the JSON shapes are exercised by
`hooks::tests` (hook_start/hook_end paths) and the SSE tests; beyond that,
manually inspect the diff to confirm no key renames.

`bash scripts/kernel-build.sh`.

**Verify**: build succeeds, binary relocked.

## Test plan

- No new behavior → no new tests required. The gate is the existing suite
  plus the greps in Done criteria.
- If you want a cheap guard: a unit test asserting
  `abort_invoke(...)` returns an error whose `to_string()` equals the input
  msg is acceptable but optional.

## Done criteria

- [ ] `grep -c "session_guard.mark_error" src/src/main.rs` → 1
- [ ] `grep -c "fabrication_check" src/src/main.rs` → reduced (single
      `log_and_emit` call site instead of two builds)
- [ ] `cargo test --release` → all pass
- [ ] `cargo clippy --release` → zero warnings
- [ ] `bash scripts/kernel-build.sh` succeeds; binary relocked
- [ ] Only `src/src/main.rs` modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- Fewer than 6 error-exit sites remain (already deduped — mark plan REJECTED
  in the index with a one-line note).
- Any site's logged vs emitted payloads differ in ways not describable as
  "kind vs type" — skip that site; if ALL sites differ, the helper isn't
  worth it: report back.
- Borrow-checker friction requires changing `SessionEndGuard`'s API — stop;
  the guard's Drop semantics are load-bearing (it fires session-end hooks).

## Maintenance notes

- Future error paths in the agent loop must call `abort_invoke` — reviewer
  should flag any new `session_guard.mark_error` outside the helper.
- The auto_nudge site (if left unconverted) is the known exception; noted
  here so nobody "cleans it up" into the helper and silently changes the
  emitted event shape.
