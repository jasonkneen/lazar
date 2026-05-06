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
use std::path::PathBuf;

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
}

impl SuspicionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SuspicionKind::PastTenseAction => "past_tense_action",
            SuspicionKind::StateTable => "state_table",
            SuspicionKind::TotalTime => "total_time",
            SuspicionKind::CheckmarkClaim => "checkmark_claim",
            SuspicionKind::Recency => "recency",
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

/// Count tool_use blocks in an assistant content array. Used to detect
/// "claims of work without any tool ran" — the strongest fabrication
/// signal.
pub fn count_tool_uses(content: &Value) -> usize {
    if let Some(arr) = content.as_array() {
        arr.iter()
            .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
            .count()
    } else {
        0
    }
}

/// Scan text for fabrication tells. Returns suspicions ordered by
/// position in the text. Empty if nothing matched.
pub fn scan(text: &str) -> Vec<Suspicion> {
    let mut suspicions = Vec::new();
    let lower = text.to_lowercase();

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
        let mut from = 0;
        while let Some(idx) = lower[from..].find(trigger) {
            let abs = from + idx;
            let after = &lower[abs + trigger.len()..];
            for verb in &action_verbs {
                if after.starts_with(verb) {
                    let end = (abs + trigger.len() + verb.len() + 40).min(text.len());
                    let mut start = abs;
                    while !text.is_char_boundary(start) && start > 0 {
                        start -= 1;
                    }
                    let mut e = end;
                    while !text.is_char_boundary(e) && e < text.len() {
                        e += 1;
                    }
                    suspicions.push(Suspicion {
                        kind: SuspicionKind::PastTenseAction,
                        matched_text: text[start..e].trim().to_string(),
                    });
                    break;
                }
            }
            from = abs + trigger.len();
        }
    }

    // 2. "Total time: ~N seconds" / "Total time: N min"
    if let Some(idx) = lower.find("total time:") {
        let end = (idx + 60).min(text.len());
        let mut e = end;
        while !text.is_char_boundary(e) && e < text.len() {
            e += 1;
        }
        suspicions.push(Suspicion {
            kind: SuspicionKind::TotalTime,
            matched_text: text[idx..e].trim().to_string(),
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
        if let Some(idx) = lower.find(&lower_phrase) {
            let end = (idx + 80).min(text.len());
            let mut e = end;
            while !text.is_char_boundary(e) && e < text.len() {
                e += 1;
            }
            suspicions.push(Suspicion {
                kind: SuspicionKind::CheckmarkClaim,
                matched_text: text[idx..e].trim().to_string(),
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
    let lower_table = lower.as_str();
    for sig in &table_signals {
        if let Some(idx) = lower_table.find(sig) {
            // Walk back to find the start of the table line
            let line_start = lower_table[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let line_end = lower_table[idx..]
                .find('\n')
                .map(|i| idx + i)
                .unwrap_or(text.len());
            let mut s = line_start;
            while !text.is_char_boundary(s) && s > 0 {
                s -= 1;
            }
            let mut e = line_end;
            while !text.is_char_boundary(e) && e < text.len() {
                e += 1;
            }
            suspicions.push(Suspicion {
                kind: SuspicionKind::StateTable,
                matched_text: text[s..e].trim().to_string(),
            });
            break; // one table cell finding is enough
        }
    }

    // 5. "Within the last N minutes/seconds" / "in the last N minutes"
    let recency_phrases = ["within the last ", "in the last "];
    for phrase in &recency_phrases {
        if let Some(idx) = lower.find(phrase) {
            let after = &lower[idx + phrase.len()..];
            // Quick check: starts with a digit?
            if after.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                let end = (idx + phrase.len() + 40).min(text.len());
                let mut e = end;
                while !text.is_char_boundary(e) && e < text.len() {
                    e += 1;
                }
                suspicions.push(Suspicion {
                    kind: SuspicionKind::Recency,
                    matched_text: text[idx..e].trim().to_string(),
                });
            }
        }
    }

    suspicions
}

/// Format a visible warning for the operator. Prepended to the
/// assistant's text before printing. Loud, hard to miss.
pub fn format_warning(suspicions: &[Suspicion], tool_uses_in_turn: usize) -> String {
    let mut out = String::new();
    out.push_str("\n");
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
        let preview = if s.matched_text.len() > 100 {
            format!("{}…", &s.matched_text[..100])
        } else {
            s.matched_text.clone()
        };
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
//   contains=<path>:<substr>  — file contains substring (head 64KB)
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

/// Extract all `[VERIFY]...[/VERIFY]` blocks from text and parse the
/// claims inside. Returns one Vec of claims (concatenated across blocks).
pub fn parse_verify_blocks(text: &str) -> Vec<VerifyClaim> {
    let mut claims = Vec::new();
    let mut from = 0;
    while let Some(start_idx) = text[from..].find("[VERIFY]") {
        let abs_start = from + start_idx + "[VERIFY]".len();
        let after = &text[abs_start..];
        let end_idx = match after.find("[/VERIFY]") {
            Some(i) => i,
            None => break,
        };
        let block = &after[..end_idx];
        for line in block.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(claim) = parse_claim_line(line) {
                claims.push(claim);
            }
        }
        from = abs_start + end_idx + "[/VERIFY]".len();
    }
    claims
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
        .trim_start_matches(|c: char| matches!(c, '-' | '*' | '•'))
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
        VerifyClaim::Size(path, n) => {
            let p = expand_path(path);
            match fs::metadata(&p) {
                Ok(m) => {
                    let len = m.len();
                    let ok = len == *n;
                    ClaimResult {
                        claim: claim.clone(),
                        passed: ok,
                        note: format!("actual size {} bytes (expected {})", len, n),
                    }
                }
                Err(e) => ClaimResult {
                    claim: claim.clone(),
                    passed: false,
                    note: format!("stat failed: {e}"),
                },
            }
        }
        VerifyClaim::SizeAtLeast(path, n) => {
            let p = expand_path(path);
            match fs::metadata(&p) {
                Ok(m) => {
                    let len = m.len();
                    let ok = len >= *n;
                    ClaimResult {
                        claim: claim.clone(),
                        passed: ok,
                        note: format!("actual size {} bytes (need >= {})", len, n),
                    }
                }
                Err(e) => ClaimResult {
                    claim: claim.clone(),
                    passed: false,
                    note: format!("stat failed: {e}"),
                },
            }
        }
        VerifyClaim::SizeAtMost(path, n) => {
            let p = expand_path(path);
            match fs::metadata(&p) {
                Ok(m) => {
                    let len = m.len();
                    let ok = len <= *n;
                    ClaimResult {
                        claim: claim.clone(),
                        passed: ok,
                        note: format!("actual size {} bytes (need <= {})", len, n),
                    }
                }
                Err(e) => ClaimResult {
                    claim: claim.clone(),
                    passed: false,
                    note: format!("stat failed: {e}"),
                },
            }
        }
        VerifyClaim::Contains(path, needle) => {
            let p = expand_path(path);
            const READ_CAP: usize = 65_536;
            match fs::read_to_string(&p) {
                Ok(s) => {
                    let head = if s.len() > READ_CAP { &s[..READ_CAP] } else { &s };
                    let ok = head.contains(needle.as_str());
                    ClaimResult {
                        claim: claim.clone(),
                        passed: ok,
                        note: if ok {
                            format!("contains '{}'", short(needle, 40))
                        } else {
                            format!("missing '{}' in head 64KB", short(needle, 40))
                        },
                    }
                }
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

/// Run every claim, return a report.
pub fn verify_all(claims: &[VerifyClaim]) -> VerifyReport {
    VerifyReport {
        claims: claims.iter().map(check_claim).collect(),
    }
}

fn expand_path(p: &PathBuf) -> PathBuf {
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
    p.clone()
}

fn short(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Format a verify report as a banner. Loud on FAIL, quiet on PASS.
pub fn format_verify_banner(report: &VerifyReport) -> String {
    if report.claims.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    if report.any_failed() {
        out.push_str("\n");
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
    fn count_tool_uses_finds_blocks() {
        let content = json!([
            {"type": "text", "text": "hello"},
            {"type": "tool_use", "name": "execute", "input": {}},
            {"type": "tool_use", "name": "execute", "input": {}},
        ]);
        assert_eq!(count_tool_uses(&content), 2);
    }

    #[test]
    fn count_tool_uses_zero_for_text_only() {
        let content = json!([{"type": "text", "text": "hi"}]);
        assert_eq!(count_tool_uses(&content), 0);
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
        let claims = parse_verify_blocks(text);
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
        let claims = parse_verify_blocks(text);
        assert_eq!(claims.len(), 2);
    }

    #[test]
    fn parse_verify_handles_multiple_blocks() {
        let text = "[VERIFY]\nexists=/a\n[/VERIFY]\nMiddle\n[VERIFY]\nexists=/b\n[/VERIFY]";
        let claims = parse_verify_blocks(text);
        assert_eq!(claims.len(), 2);
    }

    #[test]
    fn parse_verify_ignores_unknown_keys() {
        let text = "[VERIFY]\nexists=/tmp/foo\nbogus=anything\n[/VERIFY]";
        let claims = parse_verify_blocks(text);
        assert_eq!(claims.len(), 1);
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
