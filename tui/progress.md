# Display-ready core output progress

Branch: engineer/display-ready-8dbb230. Base: 8dbb230. Plan approved in chat. Implementation and local verification finished; full acceptance remains partial.

## Outputs

- src/src/core/answer.rs: bounded incremental DisplayText filter and complete-record display helper. Removes VERIFY, SPEAK and SOURCES, including inline/case-insensitive/unclosed forms.
- src/src/main.rs: stream-json text_delta now includes display_text; raw text is unchanged. Per-content-block filters finish at block completion, normal completion and interruption.
- src/src/acp.rs: live ACP chunks consume display_text; history is filtered in core. User history remains unchanged.
- src/src/jobs.rs: timeline text uses the same core filter and trims whitespace.
- tui/src/render/transcript.rs: no machine-tag parser; directly renders provided markdown.
- tui/src/render/bench_timing.rs: supplied baseline workload committed as an ordinary test, without ignore, sleeps or elapsed-time pass thresholds. Nonempty output assertions exercise rendering.
- tui/plan.md, tui/checks.md, tui/progress.md, tui/CLAUDE.md: scoped harness reports. Reports are under tui/ rather than repo root to respect edit scope.

## Measured release timing

Command: cargo test --release --manifest-path tui/Cargo.toml bench_timing -- --nocapture

Same workload: 300 turns, width 100, 200 cached frames, 5 uncached frames; 6899 rendered lines in both versions. Baseline measured with the harness registered before implementation; after measured on final saved source. One sample each; diagnostic, not a statistically established speedup or an end-to-end core-stream benchmark.

| Metric | Before (ms) | After (ms) |
| --- | ---: | ---: |
| Highlighter construction | 0.83 | 0.99 |
| First paint | 110.25 | 102.31 |
| Cached frame average | 0.881 | 0.829 |
| Uncached frame average | 81.33 | 75.07 |

Evidence: tui/before-timing.log (transcribed from captured command output), tui/after-timing.log (captured final output).

## Verification and baseline comparison

Core baseline: 181 tests, 172 passed, 9 failed. Final: 186 tests, 177 passed, same 9 failing names. Exact failure-name comparison returned exit 0. All five new core tests pass. The transient jobs timeline whitespace regression was fixed by trimming core-filtered timeline text; its final test passes.

Unchanged failing tests:
- hooks::tests::strict_hook_that_prints_transform_then_fails_vetoes
- hooks::tests::strict_hook_that_prints_transform_then_times_out_vetoes
- spawn::tests::sandbox_command_writes_inside_allowed_root_but_not_its_sibling
- tests::a_false_claim_gets_one_correction_turn
- tests::agent_loop_executes_tool_calls_then_ends_on_end_turn
- tests::nested_commands_inherit_turn_budgets
- tests::provider_overflow_compacts_and_retries_once
- tests::structured_answers_carry_reply_speech_sources_and_verification
- tests::verified_claims_pass_in_one_request

Logs show sandbox-exec sandbox_apply Operation not permitted, missing tool-created files, context compaction threshold failure, and local fixture request failures. These are observed baseline failures, not all proven to share one root cause. No production security controls or tests were weakened.

TUI baseline: 126 passed, 1 ignored. Final: 123 passed, 1 ignored; seven renderer parser-specific tests were replaced by three renderer-contract tests, and the timing test was added. There are no failing TUI names. Core/TUI cargo check pass. Core/TUI cargo clippy --all-targets exit 0 with warnings; new needless-borrow warnings were corrected. git diff --check passes. Crate-wide cargo fmt --check fails; not repaired by a broad unrelated formatting rewrite. Evidence is saved in scoped format-check.log files.

## Decisions and risks

Raw text stays available in stream-json and sessions for verification and speech extraction; clients should prefer display_text for presentation. TUI debug no longer exposes raw machine blocks through renderer parsing; raw logs remain the debugging source. Machine tags are reserved even inline, as in the prior renderer. Pending potential tag prefixes are held until resolved and flushed literally at completion if not a real block. Complete reply, job timeline and history use the same display filter.

No existing CLAUDE.md was present in the scoped directories. tui/CLAUDE.md adds reusable boundary and verification lessons. No hooks were installed and no files outside src/ and tui/ were edited.

## Open issues and next action

Full core cargo test acceptance is unresolved because the same nine baseline failures persist. Formatting acceptance is unresolved. Draft PR creation is not authorised: explicit no-push/no-PR instruction overrides the contradictory draft-PR done condition. No push, PR, merge or deployment performed. Next action is to resolve the baseline environment/test failures in separately agreed scope and obtain separate approval before any remote publication.

Code and harness reports have been committed locally on the named branch and Git status was checked clean. The final commit receipt is published separately so the committed progress file need not self-reference its own hash.