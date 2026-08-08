//! lazar fabrication verifier — kernel-level fabrication detection.
//!
//! When the agent emits `stop_reason: end_turn`, the kernel scans the
//! assistant's final text for documented fabrication tells:
//!
//!   - "✅ Just X" / "✅ Truly X" / "✅ Done — X" — checkmark+claim patterns
//!   - "Total time: ~N seconds" — outcome time-stamp reports
//!   - "Just rotated/compressed/moved/distilled/created/deleted/cleaned" — past-tense action verbs
//!   - "Within the last N minutes" — recency claims
//!   - Markdown tables with pipe-separated cells of file sizes/timestamps
//!
//! For each suspicion, the verifier checks whether THIS turn (the
//! current `end_turn` cycle) had any `tool_use` blocks at all. If the
//! text claims completed work but no tool ran, that's a hard fabrication
//! signal — the kernel prepends a visible warning to the operator
//! before emitting the response.
//!
//! Heuristic, not perfect. False positives are possible (operator
//! discussing past work in a normal conversation). False negatives are
//! certain (subtle fabrications that mix real and fake outcomes). The
//! goal is to catch the blatant cases — the table-of-fake-evidence
//! pattern that triggered the May 5 incident — not to be flawless.

use serde_json::{json, Value};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Suspicion {
    pub kind: SuspicionKind,
    pub matched_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuspicionKind {
    /// Past-tense action verb ("just rotated", "just compressed", etc.)
    PastTenseAction,
    /// Markdown table with state cells (sizes, paths, timestamps).
    StateTable,
    /// "Total time: ~N seconds" outcome-time pattern.
    TotalTime,
    /// Checkmark + completion phrase ("✅ Done", "✅ Truly").
    CheckmarkClaim,
    /// "Within the last N minutes/seconds" recency claim.
    Recency,
    /// Explicit implementation/completion + test claim.
    CompletionClaim,
}

impl SuspicionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SuspicionKind::PastTenseAction => "past_tense_action",
            SuspicionKind::StateTable => "state_table",
            SuspicionKind::TotalTime => "total_time",
            SuspicionKind::CheckmarkClaim => "checkmark_claim",
            SuspicionKind::Recency => "recency",
            SuspicionKind::CompletionClaim => "completion_claim",
        }
    }
}

/// Walk an assistant content array and return all text blocks
/// concatenated. Returns "" if content is missing or has no text.
pub fn extract_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let mut out = String::new();
    if let Some(arr) = content.as_array() {
        for block in arr {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
        }
    }
    out
}

/// Scan text for fabrication tells. Returns suspicions ordered by
/// position in the text. Empty if nothing matched.
pub fn scan(text: &str) -> Vec<Suspicion> {
    let mut suspicions = Vec::new();
    let lines: Vec<(&str, String)> = text
        .lines()
        .map(|line| (line, line.to_lowercase()))
        .collect();

    // 1. Past-tense action verbs that claim completion
    //    Pattern: "just X" / "now X" / "already X" where X is an action verb
    let action_verbs = [
        "rotated", "compressed", "moved", "deleted", "removed",
        "created", "distilled", "cleaned", "archived", "wrote",
        "saved", "persisted", "applied", "installed", "fixed",
        "patched", "updated", "rebuilt", "regenerated",
    ];
    let triggers = ["just ", "now ", "already ", "i've ", "i have ", "i ran "];
    for trigger in &triggers {
        for (line, lower) in &lines {
            let mut from = 0;
            while let Some(idx) = lower[from..].find(trigger) {
                let abs = from + idx;
                let after = &lower[abs + trigger.len()..];
                if action_verbs.iter().any(|verb| after.starts_with(verb)) {
                    suspicions.push(Suspicion {
                        kind: SuspicionKind::PastTenseAction,
                        matched_text: line.trim().to_string(),
                    });
                }
                from = abs + trigger.len();
            }
        }
    }

    // 2. "Total time: ~N seconds" / "Total time: N min"
    if let Some((line, _)) = lines.iter().find(|(_, lower)| lower.contains("total time:")) {
        suspicions.push(Suspicion {
            kind: SuspicionKind::TotalTime,
            matched_text: line.trim().to_string(),
        });
    }

    // 3. Checkmark + completion claim
    let checkmark_phrases = [
        "✅ done",
        "✅ truly",
        "✅ just",
        "✅ completed",
        "✅ verified",
        "✅ removed",
        "✅ moved",
        "✅ rotated",
        "✅ compressed",
        "✅ fresh",
    ];
    for phrase in &checkmark_phrases {
        let lower_phrase = phrase.to_lowercase();
        if let Some((line, _)) = lines.iter().find(|(_, lower)| lower.contains(&lower_phrase)) {
            suspicions.push(Suspicion {
                kind: SuspicionKind::CheckmarkClaim,
                matched_text: line.trim().to_string(),
            });
        }
    }

    // 4. Markdown table with state cells
    //    A table is a sequence of pipe-separated lines containing things
    //    that look like file sizes, timestamps, or check/cross marks.
    //    We don't try to fully parse markdown — just detect the shape.
    let table_signals = [
        "| ✅",
        "| ❌",
        "bytes |",
        "kb |",
        "mb |",
        "rotated |",
        "compressed |",
        "removed |",
        "fresh content |",
    ];
    for sig in &table_signals {
        if let Some((line, _)) = lines.iter().find(|(_, lower)| lower.contains(sig)) {
            suspicions.push(Suspicion {
                kind: SuspicionKind::StateTable,
                matched_text: line.trim().to_string(),
            });
            break; // one table cell finding is enough
        }
    }

    // 5. "Within the last N minutes/seconds" / "in the last N minutes"
    let recency_phrases = ["within the last ", "in the last "];
    for phrase in &recency_phrases {
        for (line, lower) in &lines {
            if let Some(idx) = lower.find(phrase) {
                let after = &lower[idx + phrase.len()..];
                if after.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                    suspicions.push(Suspicion {
                        kind: SuspicionKind::Recency,
                        matched_text: line.trim().to_string(),
                    });
                }
            }
        }
    }

    // 6. Explicit claims that implementation and testing are complete. These
    // require a passing VERIFY block even if they avoid the older May 5-style
    // checkmarks/tables/past-tense wording.
    let completion_phrases = [
        "implemented and tested",
        "are implemented",
        "is implemented",
        "everything is complete",
        "all work is complete",
        "fully implemented",
    ];
    for phrase in completion_phrases {
        if let Some((line, _)) = lines.iter().find(|(_, lower)| lower.contains(phrase)) {
            suspicions.push(Suspicion {
                kind: SuspicionKind::CompletionClaim,
                matched_text: line.trim().to_string(),
            });
            break;
        }
    }

    suspicions
}

/// Format a visible warning for the operator. Prepended to the
/// assistant's text before printing. Loud, hard to miss.
pub fn format_warning(suspicions: &[Suspicion], tool_uses_in_turn: usize) -> String {
    let mut out = String::new();
    out.push('\n');
    out.push_str("════════════════════════════════════════════════════════════════\n");
    out.push_str("⚠  FABRICATION CHECK — KERNEL WARNING\n");
    out.push_str("════════════════════════════════════════════════════════════════\n");
    out.push_str(&format!(
        "The assistant's response below contains {} pattern(s) that\n",
        suspicions.len()
    ));
    out.push_str("look like claims of completed work, but the model called\n");
    out.push_str(&format!(
        "{} tool(s) in this turn. Verify before trusting the response.\n\n",
        tool_uses_in_turn
    ));
    out.push_str("Suspicious phrases:\n");
    for (i, s) in suspicions.iter().enumerate().take(8) {
        let preview = short(&s.matched_text, 100);
        out.push_str(&format!("  {}. [{}] {}\n", i + 1, s.kind.as_str(), preview));
    }
    if suspicions.len() > 8 {
        out.push_str(&format!("  …and {} more\n", suspicions.len() - 8));
    }
    out.push_str("\nIf the claims above are real, run a verification command\n");
    out.push_str("(ls, stat, cat) to confirm. If they're not, the agent\n");
    out.push_str("fabricated. Either way, do NOT trust without verifying.\n");
    out.push_str("════════════════════════════════════════════════════════════════\n\n");
    out
}

/// Decide whether to flag based on suspicions and tool use count.
/// Heuristic: flag if any suspicion fired AND fewer than 2 tools ran
/// in the turn. If many tools ran, the agent probably did real work
/// and the past-tense phrasing is descriptive rather than fabricated.
pub fn should_flag(suspicions: &[Suspicion], tool_uses_in_turn: usize) -> bool {
    if suspicions.is_empty() {
        return false;
    }
    // Hardest signal: state table or checkmark claim with NO tools at all
    let has_strong_signal = suspicions.iter().any(|s| {
        matches!(
            s.kind,
            SuspicionKind::StateTable | SuspicionKind::CheckmarkClaim
        )
    });
    if has_strong_signal && tool_uses_in_turn == 0 {
        return true;
    }
    if suspicions
        .iter()
        .any(|s| s.kind == SuspicionKind::CompletionClaim)
        && tool_uses_in_turn == 0
    {
        return true;
    }
    // Multiple past-tense actions with zero or one tool — suspicious
    let past_tense_count = suspicions
        .iter()
        .filter(|s| s.kind == SuspicionKind::PastTenseAction)
        .count();
    if past_tense_count >= 2 && tool_uses_in_turn <= 1 {
        return true;
    }
    // Total time + zero tools — very suspicious
    let has_total_time = suspicions
        .iter()
        .any(|s| s.kind == SuspicionKind::TotalTime);
    if has_total_time && tool_uses_in_turn == 0 {
        return true;
    }
    false
}

/// Explain why a terminal response cannot be accepted as grounded.
///
/// Prefer specific diagnosis over a single catch-all string: operators (and the
/// model on retry) need to know whether claims failed, the block was malformed,
/// a line was invalid, or completion language lacked any VERIFY proof.
pub fn grounding_failure_reason(
    report: &VerifyReport,
    fabrication_flagged: bool,
    parsed: &ParsedVerifyBlocks,
    has_verify_marker: bool,
    has_completion_claim: bool,
) -> Option<String> {
    if report.any_failed() {
        return Some(format!(
            "kernel verification failed: {} of {} declared claims did not match filesystem state",
            report.failed_count(),
            report.claims.len()
        ));
    }
    if parsed.malformed {
        return Some(
            "VERIFY block is malformed (unbalanced [VERIFY] / [/VERIFY] markers; close must be exactly [/VERIFY])"
                .into(),
        );
    }
    if !parsed.invalid_lines.is_empty() {
        let preview = parsed
            .invalid_lines
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let extra = if parsed.invalid_lines.len() > 3 {
            format!(" (+{} more)", parsed.invalid_lines.len() - 3)
        } else {
            String::new()
        };
        return Some(format!(
            "VERIFY block has {} invalid line(s) (only exists/not_exists/size/size_at_least/size_at_most/contains/mtime_within allowed): {}{}",
            parsed.invalid_lines.len(),
            preview,
            extra
        ));
    }
    if has_verify_marker && parsed.claims.is_empty() {
        return Some(
            "VERIFY markers present but no parseable claims were found inside the block".into(),
        );
    }
    if has_completion_claim && !report.all_passed() {
        return Some(
            "completion claims require a valid, passing [VERIFY] block; the block was missing or did not all pass"
                .into(),
        );
    }
    if fabrication_flagged {
        return Some("kernel fabrication detector flagged unsupported completion claims".into());
    }
    None
}

/// Build the provider-neutral correction prompt injected after an ungrounded
/// terminal response.
pub fn format_grounding_retry_prompt(
    report: &VerifyReport,
    suspicions: &[Suspicion],
    tool_uses_in_invocation: usize,
    retry: u32,
    max_retries: u32,
) -> String {
    let mut out = String::new();
    out.push_str("KERNEL GROUNDING CORRECTION\n");
    out.push_str(&format!("Retry {retry} of {max_retries}. "));
    if report.any_failed() {
        out.push_str(&format!(
            "The kernel checked your [VERIFY] block and {} of {} claims failed.\n",
            report.failed_count(),
            report.claims.len()
        ));
        for result in report.claims.iter().filter(|result| !result.passed).take(8) {
            out.push_str(&format!("- {:?}: {}\n", result.claim, short(&result.note, 160)));
        }
        if report.failed_count() > 8 {
            out.push_str(&format!("- and {} more failed claims\n", report.failed_count() - 8));
        }
    } else if report.all_passed() {
        out.push_str(
            "Some claims in your [VERIFY] block passed, but the VERIFY contract itself is still broken (malformed markers, invalid claim lines, or completion language without a fully valid block). Fix the block shape — close with exactly [/VERIFY] on its own line, one supported claim per line, no comments.\n",
        );
    } else {
        out.push_str(
            "The kernel flagged unsupported completion language and/or a missing [VERIFY] block in your response.\n",
        );
    }
    if !suspicions.is_empty() {
        let kinds = suspicions
            .iter()
            .map(|s| s.kind.as_str())
            .collect::<std::collections::HashSet<_>>();
        out.push_str(&format!("Fabrication signals: {}.\n", kinds.into_iter().collect::<Vec<_>>().join(", ")));
    }
    out.push_str(&format!(
        "Valid bash calls observed in this invocation: {tool_uses_in_invocation}.\n"
    ));
    out.push_str(
        r#"CORRECTIVE RULES (mandatory):
1. Run bash now. Inspect real paths with rg/wc/ls - do not invent substrings from memory.
2. DROP every failed claim. Do not rephrase a failed contains= into another guessed quote.
3. Prefer exists= and size_at_least= over contains=. Use contains= only with a SHORT unique token (<=80 chars) copied from this turn's tool output.
4. Max 8 VERIFY claims. Never claim line counts, paraphrased comments, or whole-file prose matches.
5. Analysis/summary answers: omit [VERIFY] entirely and avoid past-tense completion language (I fixed / implemented and tested).
6. If you cannot prove an outcome, say so plainly without a VERIFY block.
End with either (a) a new minimal [VERIFY] that only asserts what tools just proved, or (b) no VERIFY and no success claims."#,
    );

    out
}

// ════════════════════════════════════════════════════════════════════
// VERIFY-BLOCK ENGINE — kernel-side claim checking.
//
// The model emits `[VERIFY] ... [/VERIFY]` blocks at end-of-turn to
// declare testable claims. The kernel parses these and runs actual
// filesystem checks (stat, exists, regex match) to validate. The model
// CANNOT lie about file state because the kernel does the check, not
// the model.
//
// Supported claim types:
//   exists=<path>             — file/dir exists
//   not_exists=<path>         — file/dir absent
//   size=<path>:<n>           — file size in bytes (exact)
//   size_at_least=<path>:<n>  — file size >= n bytes
//   size_at_most=<path>:<n>   — file size <= n bytes
//   contains=<path>:<substr>  — file contains substring (full file, max 8MB; needle max 160 chars)
//   mtime_within=<path>:<sec> — file mtime within last <sec> seconds
//
// One claim per line inside `[VERIFY]...[/VERIFY]`.
// ════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyClaim {
    Exists(PathBuf),
    NotExists(PathBuf),
    Size(PathBuf, u64),
    SizeAtLeast(PathBuf, u64),
    SizeAtMost(PathBuf, u64),
    Contains(PathBuf, String),
    MtimeWithin(PathBuf, u64),
}

#[derive(Debug, Clone)]
pub struct ClaimResult {
    pub claim: VerifyClaim,
    pub passed: bool,
    pub note: String,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    pub claims: Vec<ClaimResult>,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedVerifyBlocks {
    pub claims: Vec<VerifyClaim>,
    pub invalid_lines: Vec<String>,
    pub malformed: bool,
}

impl VerifyReport {
    pub fn all_passed(&self) -> bool {
        !self.claims.is_empty() && self.claims.iter().all(|r| r.passed)
    }
    pub fn any_failed(&self) -> bool {
        self.claims.iter().any(|r| !r.passed)
    }
    pub fn passed_count(&self) -> usize {
        self.claims.iter().filter(|r| r.passed).count()
    }
    pub fn failed_count(&self) -> usize {
        self.claims.iter().filter(|r| !r.passed).count()
    }
}

/// Strict VERIFY parser used by the kernel acceptance gate. Unknown or
/// malformed non-empty lines and unbalanced markers remain visible as contract
/// failures instead of being silently discarded beside a passing claim.
///
/// Close-tag note: models frequently emit `[/VERIFY` (missing the trailing
/// `]`). Treat that as a valid close — the intent is unambiguous, and rejecting
/// otherwise-passing claim sets for a one-character typo caused endemic
/// `grounding_failed` after N/N claim passes.
pub fn parse_verify_blocks_strict(text: &str) -> ParsedVerifyBlocks {
    let mut claims = Vec::new();
    let mut invalid_lines = Vec::new();
    let mut malformed = false;
    let mut in_block = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if is_verify_open_marker(line) {
            if in_block {
                malformed = true;
            } else {
                in_block = true;
            }
        } else if is_verify_close_marker(line) {
            if in_block {
                in_block = false;
            } else {
                malformed = true;
            }
        } else if line.is_empty() {
            // skip blank lines inside/outside blocks
        } else if in_block {
            if let Some(claim) = parse_claim_line(line) {
                claims.push(claim);
            } else {
                invalid_lines.push(line.chars().take(160).collect());
            }
        }
    }
    if in_block {
        malformed = true;
    }
    ParsedVerifyBlocks { claims, invalid_lines, malformed }
}

/// Canonical open marker only — do not accept prose-adjacent variants.
fn is_verify_open_marker(line: &str) -> bool {
    line == "[VERIFY]"
}

/// Accept the canonical close and the endemic truncated form `[/VERIFY`.
/// Reject anything else so junk lines stay visible as invalid_lines.
fn is_verify_close_marker(line: &str) -> bool {
    line == "[/VERIFY]" || line == "[/VERIFY"
}

/// Normalize a string before path comparison:
///   - Unicode dashes (U+2010..U+2015, U+2212) → ASCII '-'
///   - Strip markdown link wrappers like `[name](url)` → `name`
///   - Strip backtick wrappers like `` `path` `` → `path`
///   - Trim whitespace
///
/// This closes a real fabrication vector: model outputs paths with
/// non-breaking hyphens that LOOK identical to the displayed path but
/// don't match anything on the filesystem, making "not found" claims
/// pass spuriously.
fn normalize_claim_value(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for c in val.chars() {
        let mapped = match c {
            // Unicode dashes / hyphens / minus → ASCII '-'
            '\u{2010}' // hyphen
            | '\u{2011}' // non-breaking hyphen
            | '\u{2012}' // figure dash
            | '\u{2013}' // en dash
            | '\u{2014}' // em dash
            | '\u{2015}' // horizontal bar
            | '\u{2212}' // minus sign
            | '\u{FE58}' // small em dash
            | '\u{FE63}' // small hyphen-minus
            | '\u{FF0D}' // fullwidth hyphen-minus
            => '-',
            // Smart quotes → ASCII quote
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            other => other,
        };
        out.push(mapped);
    }

    // If the value contains markdown link syntax `[X](Y)`, take just X.
    // The model sometimes outputs paths through a formatter that adds links.
    if let Some(open_bracket) = out.find('[') {
        if let Some(close_bracket) = out[open_bracket..].find(']') {
            let after_close = open_bracket + close_bracket + 1;
            if out[after_close..].starts_with('(') {
                if let Some(close_paren) = out[after_close..].find(')') {
                    // Replace `[label](url)` with just `label`, keeping
                    // anything before/after.
                    let label = out[open_bracket + 1..open_bracket + close_bracket].to_string();
                    let after_url = after_close + close_paren + 1;
                    let mut rebuilt = String::new();
                    rebuilt.push_str(&out[..open_bracket]);
                    rebuilt.push_str(&label);
                    rebuilt.push_str(&out[after_url..]);
                    out = rebuilt;
                }
            }
        }
    }

    // Strip surrounding backticks if any.
    let trimmed = out.trim().trim_matches('`').trim().to_string();
    trimmed
}

fn parse_claim_line(line: &str) -> Option<VerifyClaim> {
    // Strip leading "- " or "* " in case the model writes a bullet list
    let line = line
        .trim_start_matches(['-', '*', '•'])
        .trim();
    let (key, val) = line.split_once('=')?;
    let key = key.trim();
    let val_raw = val.trim();
    let normalized = normalize_claim_value(val_raw);
    let val = normalized.as_str();
    match key {
        "exists" => Some(VerifyClaim::Exists(PathBuf::from(val))),
        "not_exists" | "absent" => Some(VerifyClaim::NotExists(PathBuf::from(val))),
        "size" => {
            let (path, n) = val.rsplit_once(':')?;
            let n: u64 = n.trim().parse().ok()?;
            Some(VerifyClaim::Size(PathBuf::from(path.trim()), n))
        }
        "size_at_least" | "size_min" => {
            let (path, n) = val.rsplit_once(':')?;
            let n: u64 = n.trim().parse().ok()?;
            Some(VerifyClaim::SizeAtLeast(PathBuf::from(path.trim()), n))
        }
        "size_at_most" | "size_max" => {
            let (path, n) = val.rsplit_once(':')?;
            let n: u64 = n.trim().parse().ok()?;
            Some(VerifyClaim::SizeAtMost(PathBuf::from(path.trim()), n))
        }
        "contains" => {
            let (path, needle) = val.split_once(':')?;
            Some(VerifyClaim::Contains(
                PathBuf::from(path.trim()),
                needle.trim().to_string(),
            ))
        }
        "mtime_within" => {
            let (path, secs) = val.rsplit_once(':')?;
            let secs: u64 = secs.trim().parse().ok()?;
            Some(VerifyClaim::MtimeWithin(PathBuf::from(path.trim()), secs))
        }
        _ => None,
    }
}

/// Shared implementation for the Size / SizeAtLeast / SizeAtMost claims:
/// stat the file, then let the caller supply the comparator and note text.
fn check_size_claim(
    claim: &VerifyClaim,
    path: &Path,
    ok: impl Fn(u64) -> bool,
    describe: impl Fn(u64) -> String,
) -> ClaimResult {
    let p = expand_path(path);
    match fs::metadata(&p) {
        Ok(m) => {
            let len = m.len();
            ClaimResult {
                claim: claim.clone(),
                passed: ok(len),
                note: describe(len),
            }
        }
        Err(e) => ClaimResult {
            claim: claim.clone(),
            passed: false,
            note: format!("stat failed: {e}"),
        },
    }
}

/// Run a single claim against the actual filesystem. Returns
/// pass/fail and a one-line note. Read-only — no side effects.
pub fn check_claim(claim: &VerifyClaim) -> ClaimResult {
    match claim {
        VerifyClaim::Exists(path) => {
            let p = expand_path(path);
            let ok = p.exists();
            ClaimResult {
                claim: claim.clone(),
                passed: ok,
                note: if ok {
                    "exists".into()
                } else {
                    format!("not found: {}", p.display())
                },
            }
        }
        VerifyClaim::NotExists(path) => {
            let p = expand_path(path);
            let ok = !p.exists();
            ClaimResult {
                claim: claim.clone(),
                passed: ok,
                note: if ok {
                    "absent".into()
                } else {
                    format!("still present: {}", p.display())
                },
            }
        }
        VerifyClaim::Size(path, n) => check_size_claim(claim, path, |len| len == *n, |len| {
            format!("actual size {} bytes (expected {})", len, n)
        }),
        VerifyClaim::SizeAtLeast(path, n) => check_size_claim(claim, path, |len| len >= *n, |len| {
            format!("actual size {} bytes (need >= {})", len, n)
        }),
        VerifyClaim::SizeAtMost(path, n) => check_size_claim(claim, path, |len| len <= *n, |len| {
            format!("actual size {} bytes (need <= {})", len, n)
        }),
        VerifyClaim::Contains(path, needle) => {
            let p = expand_path(path);
            if needle.is_empty() {
                return ClaimResult {
                    claim: claim.clone(),
                    passed: false,
                    note: "empty contains needle is not allowed".into(),
                };
            }
            if needle.chars().count() > 160 {
                return ClaimResult {
                    claim: claim.clone(),
                    passed: false,
                    note: "contains needle too long (max 160 chars) — use a short unique token from tool output".into(),
                };
            }
            match file_contains_substr(&p, needle) {
                Ok(true) => ClaimResult {
                    claim: claim.clone(),
                    passed: true,
                    note: format!("contains '{}'", short(needle, 40)),
                },
                Ok(false) => ClaimResult {
                    claim: claim.clone(),
                    passed: false,
                    note: format!("missing '{}' in file (scanned up to 8MB)", short(needle, 40)),
                },
                Err(e) => ClaimResult {
                    claim: claim.clone(),
                    passed: false,
                    note: format!("read failed: {e}"),
                },
            }
        }
        VerifyClaim::MtimeWithin(path, secs) => {
            let p = expand_path(path);
            match fs::metadata(&p).and_then(|m| m.modified()) {
                Ok(t) => match t.elapsed() {
                    Ok(elapsed) => {
                        let age = elapsed.as_secs();
                        let ok = age <= *secs;
                        ClaimResult {
                            claim: claim.clone(),
                            passed: ok,
                            note: format!("mtime {}s ago (need <= {}s)", age, secs),
                        }
                    }
                    Err(_) => ClaimResult {
                        claim: claim.clone(),
                        passed: false,
                        note: "mtime in the future (clock skew)".into(),
                    },
                },
                Err(e) => ClaimResult {
                    claim: claim.clone(),
                    passed: false,
                    note: format!("stat failed: {e}"),
                },
            }
        }
    }
}

/// Read no more than `cap` bytes from a regular, non-symlinked file. VERIFY
/// claims are model-controlled, so they must not consume streams such as
/// /dev/zero or FIFOs while the kernel checks a claim.
fn read_regular_file_head(path: &Path, cap: usize) -> io::Result<String> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "VERIFY contains requires a regular non-symlink file",
        ));
    }

    let mut file = fs::File::open(path)?.take(cap as u64);
    let mut bytes = Vec::with_capacity(cap);
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Full-file substring search for VERIFY `contains=` claims.
/// Caps scan at 8MB so a malicious/huge path cannot hang the kernel.
/// Uses overlapping windows so matches across chunk boundaries are found.
fn file_contains_substr(path: &Path, needle: &str) -> io::Result<bool> {
    use std::io::Read;
    const MAX_SCAN: u64 = 8 * 1024 * 1024;
    const CHUNK: usize = 256 * 1024;

    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "VERIFY contains requires a regular non-symlink file",
        ));
    }
    let needle_b = needle.as_bytes();
    if needle_b.is_empty() {
        return Ok(true);
    }
    let mut file = fs::File::open(path)?;
    let mut buf = vec![0u8; CHUNK];
    let mut carry: Vec<u8> = Vec::new();
    let mut scanned: u64 = 0;
    let overlap = needle_b.len().saturating_sub(1);

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let take = n.min(MAX_SCAN.saturating_sub(scanned) as usize);
        if take == 0 {
            break;
        }
        scanned += take as u64;
        let chunk = &buf[..take];
        // Search in carry+chunk
        let mut window = Vec::with_capacity(carry.len() + chunk.len());
        window.extend_from_slice(&carry);
        window.extend_from_slice(chunk);
        if window.windows(needle_b.len()).any(|w| w == needle_b) {
            return Ok(true);
        }
        // Keep tail for boundary matches
        if window.len() > overlap {
            carry = window[window.len() - overlap..].to_vec();
        } else {
            carry = window;
        }
        if scanned >= MAX_SCAN {
            break;
        }
    }
    Ok(false)
}

/// Run every claim, return a report.
pub fn verify_all(claims: &[VerifyClaim]) -> VerifyReport {
    VerifyReport {
        claims: claims.iter().map(check_claim).collect(),
    }
}

fn expand_path(p: &Path) -> PathBuf {
    if let Some(s) = p.to_str() {
        if let Some(rest) = s.strip_prefix("~/") {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home).join(rest);
            }
        }
        if s == "~" {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home);
            }
        }
        if let Some(rest) = s.strip_prefix("$LAZAR_HOME/") {
            if let Ok(home) = std::env::var("LAZAR_HOME") {
                return PathBuf::from(home).join(rest);
            }
        }
    }
    p.to_path_buf()
}

fn short(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", head_bytes(s, max))
    }
}

/// Truncate to at most `cap` bytes without splitting a UTF-8 sequence.
/// Slicing at a raw byte index panics when the index lands inside a
/// multibyte char (e.g. '─' from TUI banners at the 64KB read cap).
pub(crate) fn head_bytes(s: &str, cap: usize) -> &str {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Format a verify report as a banner. Loud on FAIL, quiet on PASS.
pub fn format_verify_banner(report: &VerifyReport) -> String {
    if report.claims.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    if report.any_failed() {
        out.push('\n');
        out.push_str("════════════════════════════════════════════════════════════════\n");
        out.push_str(&format!(
            "❌ VERIFY FAILED — {} of {} kernel-side checks failed\n",
            report.failed_count(),
            report.claims.len()
        ));
        out.push_str("════════════════════════════════════════════════════════════════\n");
        for r in &report.claims {
            let mark = if r.passed { "✓" } else { "✗" };
            out.push_str(&format!("  {} {:?} — {}\n", mark, r.claim, r.note));
        }
        out.push_str("\nThe assistant declared these claims in a [VERIFY] block.\n");
        out.push_str("The kernel ran the actual filesystem checks. Failed claims\n");
        out.push_str("indicate the assistant's claim of completed work is wrong.\n");
        out.push_str("════════════════════════════════════════════════════════════════\n");
    } else {
        out.push_str(&format!(
            "\n  ✓ verify: {}/{} claims passed (kernel-checked)\n",
            report.passed_count(),
            report.claims.len()
        ));
    }
    out
}

/// Build a structured event payload describing the verify report.
pub fn report_to_event(report: &VerifyReport) -> Value {
    let claims_json: Vec<Value> = report
        .claims
        .iter()
        .map(|r| {
            json!({
                "claim": format!("{:?}", r.claim),
                "passed": r.passed,
                "note": r.note,
            })
        })
        .collect();
    json!({
        "passed_count": report.passed_count(),
        "failed_count": report.failed_count(),
        "all_passed": report.all_passed(),
        "any_failed": report.any_failed(),
        "claims": claims_json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scans_past_tense_actions() {
        let text = "I just rotated stream.jsonl and now compressed all archives.";
        let suspicions = scan(text);
        assert!(suspicions
            .iter()
            .any(|s| s.kind == SuspicionKind::PastTenseAction));
    }

    #[test]
    fn contains_reader_limits_regular_file_reads() {
        let path = std::env::temp_dir().join(format!(
            "lazar-verifier-cap-{}-{}",
            std::process::id(),
            crate::now_millis()
        ));
        fs::write(&path, "a".repeat(128)).unwrap();

        let head = read_regular_file_head(&path, 64).unwrap();
        assert_eq!(head.len(), 64);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn scans_total_time_pattern() {
        let text = "Total time: ~30 seconds. Workspace is clean.";
        let suspicions = scan(text);
        assert!(suspicions.iter().any(|s| s.kind == SuspicionKind::TotalTime));
    }

    #[test]
    fn scans_checkmark_claim() {
        let text = "Result: ✅ Truly rotated";
        let suspicions = scan(text);
        assert!(suspicions
            .iter()
            .any(|s| s.kind == SuspicionKind::CheckmarkClaim));
    }

    #[test]
    fn scans_state_table() {
        let text = "| Check | Result |\n|---|---|\n| stream.jsonl | 0 bytes |";
        let suspicions = scan(text);
        assert!(suspicions
            .iter()
            .any(|s| s.kind == SuspicionKind::StateTable));
    }

    #[test]
    fn scans_recency_claim() {
        let text = "All updated within the last 5 minutes.";
        let suspicions = scan(text);
        assert!(suspicions.iter().any(|s| s.kind == SuspicionKind::Recency));
    }

    #[test]
    fn benign_text_produces_no_suspicions() {
        let text = "Hello! Let me know what you'd like to work on.";
        let suspicions = scan(text);
        assert!(suspicions.is_empty());
    }

    #[test]
    fn should_flag_state_table_with_no_tools() {
        let suspicions = vec![Suspicion {
            kind: SuspicionKind::StateTable,
            matched_text: "table".into(),
        }];
        assert!(should_flag(&suspicions, 0));
    }

    #[test]
    fn should_not_flag_when_many_tools_ran() {
        let suspicions = vec![Suspicion {
            kind: SuspicionKind::PastTenseAction,
            matched_text: "just rotated".into(),
        }];
        // 5 tools ran, model is describing real work — don't flag
        assert!(!should_flag(&suspicions, 5));
    }

    #[test]
    fn should_flag_total_time_with_zero_tools() {
        let suspicions = vec![Suspicion {
            kind: SuspicionKind::TotalTime,
            matched_text: "Total time: ~30 seconds".into(),
        }];
        assert!(should_flag(&suspicions, 0));
    }

    #[test]
    fn failed_verify_requires_grounding_correction() {
        let claims = (0..16)
            .map(|i| ClaimResult {
                claim: VerifyClaim::Exists(PathBuf::from(format!("/tmp/fabricated-{i}"))),
                passed: false,
                note: "not found".into(),
            })
            .collect();
        let report = VerifyReport { claims };

        assert_eq!(
            grounding_failure_reason(
                &report,
                false,
                &ParsedVerifyBlocks::default(),
                false,
                false,
            )
            .as_deref(),
            Some("kernel verification failed: 16 of 16 declared claims did not match filesystem state")
        );
        let prompt = format_grounding_retry_prompt(&report, &[], 0, 1, 1);
        assert!(prompt.contains("16 of 16 claims failed"));
        assert!(prompt.contains("/tmp/fabricated-0"));
        assert!(prompt.contains("and 8 more failed claims"));
        assert!(prompt.contains("DROP every failed claim"));
        assert!(prompt.contains("Valid bash calls observed in this invocation: 0"));
    }

    #[test]
    fn explicit_implemented_and_tested_claim_requires_proof() {
        let text = "All 16 desktop notification files are implemented and tested.";
        let suspicions = scan(text);

        assert!(suspicions
            .iter()
            .any(|s| s.kind == SuspicionKind::CompletionClaim));
        assert!(should_flag(&suspicions, 0));
        assert_eq!(
            grounding_failure_reason(
                &VerifyReport::default(),
                true,
                &ParsedVerifyBlocks::default(),
                false,
                true,
            )
            .as_deref(),
            Some("completion claims require a valid, passing [VERIFY] block; the block was missing or did not all pass")
        );
    }

    #[test]
    fn truncated_close_marker_is_accepted() {
        // Endemic model typo from stream.jsonl: closes with `[/VERIFY` not `[/VERIFY]`.
        let text = "[VERIFY]\nexists=/tmp\nsize_at_least=/tmp:0\n[/VERIFY";
        let parsed = parse_verify_blocks_strict(text);
        assert!(!parsed.malformed, "truncated close must not mark the block malformed");
        assert!(parsed.invalid_lines.is_empty());
        assert_eq!(parsed.claims.len(), 2);
    }

    #[test]
    fn truncated_close_with_passing_claims_is_not_a_grounding_failure() {
        let text = "[VERIFY]\nexists=/tmp\n[/VERIFY";
        let parsed = parse_verify_blocks_strict(text);
        let report = verify_all(&parsed.claims);
        assert!(report.all_passed());
        assert!(
            grounding_failure_reason(&report, false, &parsed, true, false).is_none(),
            "passing claims + truncated close must accept the turn"
        );
    }

    #[test]
    fn invalid_lines_get_a_specific_grounding_reason() {
        let text = "[VERIFY]\nexists=/tmp\nbogus=nope\n[/VERIFY]";
        let parsed = parse_verify_blocks_strict(text);
        let report = verify_all(&parsed.claims);
        let reason = grounding_failure_reason(&report, false, &parsed, true, false);
        assert!(
            reason
                .as_deref()
                .unwrap_or("")
                .contains("invalid line"),
            "got: {reason:?}"
        );
        assert!(reason.as_deref().unwrap_or("").contains("bogus=nope"));
    }

    #[test]
    fn completion_scan_is_safe_after_unicode_case_expansion() {
        let text = "İİİ All files are implemented and tested.";
        let suspicions = scan(text);
        assert!(suspicions.iter().any(|s| s.kind == SuspicionKind::CompletionClaim));
    }

    #[test]
    fn extract_text_concatenates_text_blocks() {
        let content = json!([
            {"type": "text", "text": "first"},
            {"type": "tool_use", "name": "execute", "input": {}},
            {"type": "text", "text": "second"},
        ]);
        let extracted = extract_text(&content);
        assert!(extracted.contains("first"));
        assert!(extracted.contains("second"));
    }

    #[test]
    fn warning_format_is_loud() {
        let suspicions = vec![Suspicion {
            kind: SuspicionKind::CheckmarkClaim,
            matched_text: "✅ Truly rotated".into(),
        }];
        let warning = format_warning(&suspicions, 0);
        assert!(warning.contains("FABRICATION CHECK"));
        assert!(warning.contains("KERNEL WARNING"));
        assert!(warning.contains("✅ Truly rotated"));
    }

    #[test]
    fn parse_verify_block_basic() {
        let text = "Some text\n[VERIFY]\nexists=/tmp/foo\nsize=/tmp/bar:0\n[/VERIFY]\nMore text.";
        let claims = parse_verify_blocks_strict(text).claims;
        assert_eq!(claims.len(), 2);
        assert!(matches!(claims[0], VerifyClaim::Exists(_)));
        match &claims[1] {
            VerifyClaim::Size(p, n) => {
                assert_eq!(p.to_str().unwrap(), "/tmp/bar");
                assert_eq!(*n, 0);
            }
            _ => panic!("expected Size claim"),
        }
    }

    #[test]
    fn parse_verify_block_with_bullets() {
        let text = "[VERIFY]\n- exists=/tmp/x\n* not_exists=/tmp/y\n[/VERIFY]";
        let claims = parse_verify_blocks_strict(text).claims;
        assert_eq!(claims.len(), 2);
    }

    #[test]
    fn parse_verify_handles_multiple_blocks() {
        let text = "[VERIFY]\nexists=/a\n[/VERIFY]\nMiddle\n[VERIFY]\nexists=/b\n[/VERIFY]";
        let claims = parse_verify_blocks_strict(text).claims;
        assert_eq!(claims.len(), 2);
    }

    #[test]
    fn strict_verify_rejects_unknown_lines_beside_valid_claims() {
        let text = "[VERIFY]\nexists=/tmp/foo\nbogus=anything\n[/VERIFY]";
        let parsed = parse_verify_blocks_strict(text);
        assert_eq!(parsed.claims.len(), 1);
        assert_eq!(parsed.invalid_lines, vec!["bogus=anything"]);
        assert!(!parsed.malformed);
    }

    #[test]
    fn strict_verify_rejects_unbalanced_markers() {
        let parsed = parse_verify_blocks_strict("[VERIFY]\nexists=/tmp/foo");
        assert!(parsed.malformed);
    }

    #[test]
    fn strict_verify_ignores_marker_mentions_outside_the_terminal_block() {
        let text = "The `[VERIFY]` contract proves filesystem claims.\n\n\
                    [VERIFY]\nexists=/tmp\n[/VERIFY]";
        let parsed = parse_verify_blocks_strict(text);

        assert!(!parsed.malformed, "a prose marker mention must not unbalance the real block");
        assert!(parsed.invalid_lines.is_empty());
        assert_eq!(parsed.claims.len(), 1);
    }

    #[test]
    fn check_exists_passes_for_real_path() {
        let claim = VerifyClaim::Exists(PathBuf::from("/tmp"));
        let result = check_claim(&claim);
        assert!(result.passed);
    }

    #[test]
    fn check_exists_fails_for_fake_path() {
        let claim = VerifyClaim::Exists(PathBuf::from("/this/does/not/exist/lazar"));
        let result = check_claim(&claim);
        assert!(!result.passed);
    }

    #[test]
    fn check_size_validates_file_length() {
        let path = std::env::temp_dir().join(format!(
            "lazar-verify-test-{}-{}",
            std::process::id(),
            now_nanos_local()
        ));
        std::fs::write(&path, b"hello").unwrap();
        let pass = check_claim(&VerifyClaim::Size(path.clone(), 5));
        let fail = check_claim(&VerifyClaim::Size(path.clone(), 99));
        std::fs::remove_file(&path).ok();
        assert!(pass.passed);
        assert!(!fail.passed);
    }

    #[test]
    fn check_contains_finds_substring() {
        let path = std::env::temp_dir().join(format!(
            "lazar-contains-test-{}-{}",
            std::process::id(),
            now_nanos_local()
        ));
        std::fs::write(&path, b"hello lazar world").unwrap();
        let pass = check_claim(&VerifyClaim::Contains(path.clone(), "lazar".into()));
        let fail = check_claim(&VerifyClaim::Contains(path.clone(), "missing".into()));
        std::fs::remove_file(&path).ok();
        assert!(pass.passed);
        assert!(!fail.passed);
    }

    #[test]
    #[test]
    fn contains_finds_match_beyond_64kb() {
        let path = std::env::temp_dir().join(format!(
            "lazar-contains-deep-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut data = vec![b'x'; 70_000];
        data.extend_from_slice(b"UNIQUE_TOKEN_DEEP_IN_FILE");
        std::fs::write(&path, &data).unwrap();
        let claim = VerifyClaim::Contains(path.clone(), "UNIQUE_TOKEN_DEEP_IN_FILE".into());
        let result = check_claim(&claim);
        let _ = std::fs::remove_file(&path);
        assert!(result.passed, "note={}", result.note);
    }

    #[test]
    fn contains_rejects_overlong_needle() {
        let path = std::env::temp_dir().join(format!(
            "lazar-contains-long-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"hello").unwrap();
        let needle = "n".repeat(200);
        let claim = VerifyClaim::Contains(path.clone(), needle);
        let result = check_claim(&claim);
        let _ = std::fs::remove_file(&path);
        assert!(!result.passed);
        assert!(result.note.contains("too long"));
    }


    fn contains_claim_survives_multibyte_char_at_read_cap() {
        // Regression: a '─' straddling the 64KB cap panicked
        // ("end byte index 65536 is not a char boundary").
        let path = std::env::temp_dir().join(format!(
            "lazar-contains-mb-test-{}-{}",
            std::process::id(),
            now_nanos_local()
        ));
        let mut content = "x".repeat(65_534);
        content.push('─'); // 3 bytes: occupies 65534..65537
        content.push_str("tail marker");
        std::fs::write(&path, content).unwrap();
        let result = check_claim(&VerifyClaim::Contains(path.clone(), "x".into()));
        std::fs::remove_file(&path).ok();
        assert!(result.passed);
    }

    #[test]
    fn head_bytes_backs_off_to_char_boundary() {
        let mut s = "x".repeat(65_534);
        s.push('─');
        assert_eq!(head_bytes(&s, 65_536).len(), 65_534);
        assert_eq!(head_bytes("ab", 10), "ab");
    }

    #[test]
    fn short_never_splits_multibyte() {
        let s = "aaaa─bbbb"; // '─' spans bytes 4..7
        let t = short(s, 5);
        assert!(t.starts_with("aaaa"));
        assert!(t.ends_with('…'));
    }

    fn now_nanos_local() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    #[test]
    fn verify_report_all_passed_works() {
        let r = VerifyReport {
            claims: vec![ClaimResult {
                claim: VerifyClaim::Exists(PathBuf::from("/tmp")),
                passed: true,
                note: "ok".into(),
            }],
        };
        assert!(r.all_passed());
    }

    #[test]
    fn normalize_dashes_replaces_unicode_hyphens() {
        // U+2011 non-breaking hyphen
        let normalized = normalize_claim_value("/path/tui\u{2011}build/x");
        assert_eq!(normalized, "/path/tui-build/x");
        // U+2013 en dash
        let normalized = normalize_claim_value("/path/foo\u{2013}bar");
        assert_eq!(normalized, "/path/foo-bar");
    }

    #[test]
    fn normalize_strips_markdown_link_syntax() {
        let normalized = normalize_claim_value("/path/[file.sh](http://file.sh)");
        assert_eq!(normalized, "/path/file.sh");
    }

    #[test]
    fn normalize_strips_backticks() {
        let normalized = normalize_claim_value("`/path/x`");
        assert_eq!(normalized, "/path/x");
    }

    #[test]
    fn parse_claim_with_unicode_hyphen_normalizes() {
        let line = "exists=/path/tui\u{2011}build/x";
        let claim = parse_claim_line(line).expect("should parse");
        match claim {
            VerifyClaim::Exists(p) => {
                assert_eq!(p.to_str().unwrap(), "/path/tui-build/x");
            }
            _ => panic!("expected Exists claim"),
        }
    }

    #[test]
    fn parse_claim_with_markdown_link_normalizes() {
        let line = "exists=/path/[build.sh](http://build.sh)";
        let claim = parse_claim_line(line).expect("should parse");
        match claim {
            VerifyClaim::Exists(p) => {
                assert_eq!(p.to_str().unwrap(), "/path/build.sh");
            }
            _ => panic!("expected Exists claim"),
        }
    }

    /// Regression test for the May 5 incident — the exact phrases lazar
    /// fabricated must trigger the verifier.
    #[test]
    fn may5_incident_phrases_are_caught() {
        let text = "Total time: ~30 seconds. Workspace is clean.";
        let suspicions = scan(text);
        assert!(should_flag(&suspicions, 0), "May 5 'Total time' phrase should flag with no tools");

        let text2 = "I just rotated the log and just compressed all archives.";
        let suspicions = scan(text2);
        assert!(should_flag(&suspicions, 0), "Multiple past-tense with no tools should flag");

        let text3 = "| Check | Result |\n|---|---|\n| stream.jsonl size | 0 bytes (was 8.2M) | ✅ Truly rotated |";
        let suspicions = scan(text3);
        assert!(should_flag(&suspicions, 0), "State-table fabrication should flag");
    }
}
