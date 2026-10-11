# Scoped display protocol harness

This file documents this run without editing outside src/ and tui/.

## Boundary rules

- Core owns VERIFY, SPEAK and SOURCES machine-block interpretation. TUI rendering consumes display-ready text and never parses those tags.
- Keep raw model text for verification, speech/source extraction and saved sessions. Stream-json text_delta carries raw text plus display_text; presentation clients must honour display_text even when it is empty.
- Streaming filters are per content-block index. Test arbitrary chunk boundaries, partial open/close tags, inline and mixed-case blocks, UTF-8, and completion/interruption tails without sleeps.
- Use the same core display helper for complete replies, history and job timelines. Preserve user text literally.
- After filtering timeline text, trim before checking emptiness. Removing tags does not automatically remove whitespace; failing to trim introduced an extra empty timeline step during this run.
- Renderer debug mode cannot reconstruct stripped raw text. Inspect raw logs for machine-block debugging instead.

## Reusable checks and scripts

No new hooks or scripts installed. Existing Cargo checks used:

```sh
cargo test --manifest-path src/Cargo.toml
cargo test --manifest-path tui/Cargo.toml
cargo test --release --manifest-path tui/Cargo.toml bench_timing -- --nocapture
cargo check --manifest-path src/Cargo.toml
cargo check --manifest-path tui/Cargo.toml
cargo clippy --manifest-path src/Cargo.toml --all-targets
cargo clippy --manifest-path tui/Cargo.toml --all-targets
cargo fmt --manifest-path src/Cargo.toml -- --check
cargo fmt --manifest-path tui/Cargo.toml -- --check
git diff --check
```

Timing tests exercise rendering with nonempty assertions; elapsed values are diagnostic, never pass thresholds. Compare baseline and final failing test names, not counts. Keep logs under approved scope and use pipefail when tee captures test output. Run git diff BASE HEAD --check after committing new logs: unstaged git diff --check misses untracked files, and trailing blank log lines can otherwise slip into the commit. A final successful shell command does not prove earlier tests passed.

## Permission and completion rules

Only src/ and tui/ edits are authorised. Reports belong under tui/. No push, PR, merge or deployment without separate approval. Full core tests currently have nine baseline failures; do not claim a green suite or bypass sandbox security to silence them. Crate-wide formatting remains failing and must not be represented as passing.