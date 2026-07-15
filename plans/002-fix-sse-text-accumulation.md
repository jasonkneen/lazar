# Plan 002: Fix O(n²) text accumulation in the SSE streaming parser

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- src/src/main.rs`
> If `src/src/main.rs` changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, STOP.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/001 (ordering only — no code dependency)
- **Category**: perf
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

`parse_sse_reader` in `src/src/main.rs` accumulates assistant text by cloning
the ENTIRE accumulated string out of the JSON block on every `text_delta`
event and reallocating (`current + text`). A response streamed as thousands of
~30-byte deltas therefore does O(n²) work: a 60KB response shuffles roughly
60MB through memcpy, inline in the stream-read loop; raising `LAZAR_MAX_TOKENS`
(default 16384, env-tunable) makes it quadratically worse. The file already
contains the correct pattern two branches down: tool inputs accumulate in a
`HashMap<u64, String>` (`tool_input_buffers`) and are folded into the block
once at `content_block_stop`. This plan applies the same pattern to text.

## Current state

- `src/src/main.rs:1092-1264` — `parse_sse_reader<R: BufRead>`, the SSE parser.
- The hot spot, `src/src/main.rs:1147-1150` (inside the `"text_delta"` arm):

```rust
if let Some(block) = blocks.get_mut(&idx) {
    let current = block["text"].as_str().unwrap_or("").to_string();
    block["text"] = json!(current + text);
}
```

- The existing correct pattern for tool input, `src/src/main.rs:1152-1155`:

```rust
"input_json_delta" => {
    let partial = delta["partial_json"].as_str().unwrap_or("");
    tool_input_buffers.entry(idx).or_default().push_str(partial);
}
```

- Buffers are declared at `src/src/main.rs:1098-1099`:

```rust
let mut blocks: HashMap<u64, Value> = HashMap::new();
let mut tool_input_buffers: HashMap<u64, String> = HashMap::new();
```

- CRITICAL subtlety — the early-EOF recovery path at
  `src/src/main.rs:1222-1250`: when the SSE socket closes before
  `message_stop` but after a `stop_reason` was seen (a real MiniMax behavior),
  the function assembles whatever is in `blocks` at lines 1256-1263. Today the
  partial text is already inside `block["text"]` because it's updated per
  delta. After this change, text lives in a side buffer until
  `content_block_stop` — so **any remaining buffered text must be folded into
  its block before final assembly**, or the recovery path silently drops text.
- Live streaming to stdout (`print!("{text}")` at line 1132, Text mode; the
  `emit_event` at 1137-1141, StreamJson mode) must be unchanged — only the
  accumulation strategy changes.
- Conventions: tests live in `#[cfg(test)] mod tests` at the bottom of
  `main.rs`; SSE parser tests use `Cursor` over byte literals — model new
  tests after `sse_parser_recovers_eof_after_stop_reason`
  (`src/src/main.rs:2376-2389`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unlock source | `bash scripts/kernel-unlock.sh` | prints "done", src/ writable |
| Tests | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release` | all pass (76 existing + new) |
| Lint | `cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo clippy --release` | zero warnings |
| Rebuild + relock | `bash scripts/kernel-build.sh` | "Kernel rebuilt and locked." |

Note: this repo IS a live install. `src/` is chmod a-w until unlocked;
`bash scripts/kernel-unlock.sh` is mandatory before editing and
`bash scripts/kernel-build.sh` re-locks when you finish.

## Scope

**In scope**:
- `src/src/main.rs` — `parse_sse_reader` and new tests in its `mod tests`.

**Out of scope**:
- The Text-mode live `print!` / StreamJson `emit_event` behavior.
- `tool_input_buffers` handling — already correct.
- Any other function in `main.rs`.

## Git workflow

- Branch: `advisor/002-sse-text-buffers` off `main`.
- Suggested commit message: `perf(kernel): accumulate SSE text deltas in side buffers (fix O(n²))`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Unlock the source

`bash scripts/kernel-unlock.sh`

**Verify**: `test -w src/src/main.rs && echo writable` → `writable`

### Step 2: Add a text buffer map and rewrite the text_delta arm

In `parse_sse_reader`, next to `tool_input_buffers` (line 1099), add:

```rust
let mut text_buffers: HashMap<u64, String> = HashMap::new();
```

Replace the block-clone at lines 1147-1150 with:

```rust
text_buffers.entry(idx).or_default().push_str(text);
```

(The `print!`/`emit_event` lines above it stay exactly as they are.)

### Step 3: Fold text at content_block_stop

In the `"content_block_stop"` arm (lines 1159-1200), where the block type is
`"text"`, fold the buffer in before the StreamJson `text_done` emit:

```rust
} else if block["type"] == "text" {
    if let Some(buf) = text_buffers.remove(&idx) {
        block["text"] = json!(buf);
    }
    if format == OutputFormat::StreamJson {
        emit_event(json!({ "type": "text_done", "index": idx }));
    }
}
```

Note: `content_block_start` seeds `block["text"]` with the API's initial value
(normally `""`). Overwriting with the buffer is correct because every
`text_delta` for that index went into the buffer.

### Step 4: Fold any unflushed buffers before final assembly

Immediately before the final assembly (`let mut indices: Vec<u64> = ...`,
line 1256), add:

```rust
// Early-EOF recovery: fold text that never saw content_block_stop.
for (idx, buf) in text_buffers.drain() {
    if let Some(block) = blocks.get_mut(&idx) {
        if block["type"] == "text" {
            block["text"] = json!(buf);
        }
    }
}
```

**Verify (steps 2–4 together)**:
`cd src && CARGO_TARGET_DIR="$HOME/lazar/workspace/.cargo-target" cargo test --release`
→ all 76 existing tests pass (especially `sse_parser_recovers_eof_after_stop_reason`).

### Step 5: Add regression tests

In `mod tests`, modeled on `sse_parser_recovers_eof_after_stop_reason`:

1. `sse_parser_accumulates_multiple_text_deltas` — one text block, three
   deltas (`"a"`, `"b"`, `"c"`), a `content_block_stop`, `message_delta` with
   stop_reason, `message_stop`. Assert `content[0]["text"] == "abc"`.
2. `sse_parser_keeps_partial_text_without_block_stop` — deltas but NO
   `content_block_stop`, then `message_delta` with `stop_reason`, then EOF
   (no `message_stop`). Assert the returned content still contains the
   concatenated partial text (this is the recovery-path guarantee from Step 4).

**Verify**: same test command → all pass including the 2 new tests.

### Step 6: Lint, rebuild, relock

`cargo clippy --release` → zero warnings, then `bash scripts/kernel-build.sh`.

**Verify**: `ls -lO bin/lazar | grep -q uchg && echo locked` → `locked`

## Test plan

Covered in Step 5. Pattern file: `src/src/main.rs` `mod tests`, tests
`sse_parser_recovers_eof_after_stop_reason` / `sse_parser_rejects_eof_without_stop_reason`.
Cases: multi-delta accumulation (happy path), early-EOF partial text (the
regression this plan must not introduce).

## Done criteria

- [ ] `grep -n "current + text" src/src/main.rs` → no matches
- [ ] `cargo test --release` → all pass, including the 2 new tests
- [ ] `cargo clippy --release` → zero warnings
- [ ] `bash scripts/kernel-build.sh` succeeds; `bin/lazar` relocked (uchg)
- [ ] Only `src/src/main.rs` modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- The code at lines 1147-1150 doesn't match the excerpt (drift).
- `sse_parser_recovers_eof_after_stop_reason` fails after Step 4 — the
  recovery fold is wrong; report rather than reshaping the recovery logic.
- The fix appears to require touching `emit_event` or Text-mode printing.

## Maintenance notes

- If a future change adds new content-block types with incremental deltas
  (e.g. thinking blocks), give them their own side buffer — never accumulate
  inside the `Value`.
- Reviewer: check that Text-mode output is byte-identical (the live `print!`
  path is untouched) and that the early-EOF fold covers blocks with no
  `content_block_stop`.
