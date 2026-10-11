# Display-ready core output plan

Status: approved in chat; implementation and local checks complete, full acceptance partial due to baseline core failures and formatting. Base: 8dbb230. No push, PR creation, or merge authorised.

All edits remain under src/ and tui/. Harness reports live in tui/ to respect scope; no root-level CLAUDE.md or progress.md edits.

1. Inspect core event emission and TUI ingestion/rendering. Read src/src/, tui/src/, and both Cargo.toml files. Create a new branch from 8dbb230 after approval. Check: clean base and documented event contract.
2. Register the supplied /tmp/lazar-baseline/tui/src/render/bench_timing.rs under tui/src/render/. Run baseline tests for both crates and release timing. Check: record actual output and failing test names in tui/progress.md. Timing must not use sleeps or timing-based pass thresholds.
3. Move verify/speak stripping to core's display event boundary; remove tag parsing from tui/src/render/transcript.rs. Preserve raw text where verification and speech processing require it. Add regression tests for streaming boundaries and final/history output as applicable. Check: core emits display-ready text and renderer has no verify/speak parser.
4. Run crate tests, formatting and applicable lint/type checks; run the same release timing harness after changes. Write tui/checks.md, tui/progress.md and scoped tui/CLAUDE.md lessons. Check: compare failing test names against base and record measured before/after values.
5. Review diff, commit only scoped changes on the new branch, and publish reports through publishProjectArtifact. Check: commit exists, working tree is clean, no push or merge. Draft PR remains unresolved until separately authorised.

Steps are sequential because baseline timing must precede code changes. Maximum three correction attempts per failed step. Never weaken checks to obtain a pass.