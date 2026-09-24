//! Grounded verification: the model's claims about external state are
//! checked against that state by the kernel, never taken on its word.
//!
//! A turn that asserts outcomes ends with a block of machine-checkable claims:
//!
//! ```text
//! [VERIFY]
//! exists=/tmp/report.md
//! contains=/tmp/report.md:## Summary
//! [/VERIFY]
//! ```
//!
//! The kernel runs every claim with real filesystem calls. A failure earns one
//! correction turn that names exactly what did not hold; a second failure ends
//! the turn as `grounding_failed`. Turns that assert nothing carry no block and
//! pass untouched. The same checker backs `lazar verify` so spawned sub-agents
//! and scripts share one claim language and one implementation.
//!
//! Deliberately deterministic: no prose classification, no phrase lists.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Claims per block. Enough to prove a multi-file change; small enough that a
/// block stays a proof, not a dump of every measurement.
pub(crate) const MAX_CLAIMS: usize = 12;
/// Longest `contains=` needle. A proof token, not a paragraph.
pub(crate) const MAX_NEEDLE_CHARS: usize = 200;
/// Correction turns before a failed block ends the turn.
pub(crate) const MAX_CORRECTIONS: u32 = 1;
/// Largest file `contains=` will read.
const MAX_CONTAINS_FILE_BYTES: u64 = 32 * 1024 * 1024;

pub(crate) const KEYS: &str =
    "exists, not_exists, size, size_at_least, size_at_most, contains, mtime_within";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Claim {
    Exists(PathBuf),
    NotExists(PathBuf),
    Size(PathBuf, u64),
    SizeAtLeast(PathBuf, u64),
    SizeAtMost(PathBuf, u64),
    Contains(PathBuf, String),
    MtimeWithin(PathBuf, u64),
}

/// One checked claim, as logged and shown to the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub claim: String,
    pub ok: bool,
    pub detail: String,
}

impl Outcome {
    pub(crate) fn to_json(&self) -> Value {
        json!({"claim": self.claim, "ok": self.ok, "detail": self.detail})
    }
}

/// Normalise one claim line: bullets, backticks and surrounding space go.
fn clean_line(line: &str) -> &str {
    let line = line.trim();
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line)
        .trim();
    line.trim_matches('`').trim()
}

fn is_open_tag(line: &str) -> bool {
    line.trim().eq_ignore_ascii_case("[verify]")
}

fn is_close_tag(line: &str) -> bool {
    let line = line.trim().to_ascii_lowercase();
    line == "[/verify]" || line == "[/verify"
}

/// The claim lines of the last `[VERIFY]` block in `text`.
///
/// `None`: no block, so nothing was asserted. `Some(Err)`: a block the kernel
/// cannot check (unclosed, empty, too many claims) — itself a failed proof.
pub(crate) fn extract_block(text: &str) -> Option<Result<Vec<String>, String>> {
    let lines: Vec<&str> = text.lines().collect();
    let open = lines.iter().rposition(|l| is_open_tag(l))?;
    let mut claims = Vec::new();
    let mut closed = false;
    for line in &lines[open + 1..] {
        if is_close_tag(line) {
            closed = true;
            break;
        }
        let line = clean_line(line);
        if !line.is_empty() {
            claims.push(line.to_string());
        }
    }
    Some(if !closed {
        Err("the [VERIFY] block is not closed with [/VERIFY]".into())
    } else if claims.is_empty() {
        Err("the [VERIFY] block is empty; omit the block when you assert nothing".into())
    } else if claims.len() > MAX_CLAIMS {
        Err(format!(
            "the [VERIFY] block has {} claims; the limit is {MAX_CLAIMS}. Keep the claims that prove the outcome",
            claims.len()
        ))
    } else {
        Ok(claims)
    })
}

/// Resolve a claim path: `~`, `$HOME` and `$LAZAR_HOME` expand; relative
/// paths are taken from the tool working directory.
fn resolve(path: &str, cwd: &Path) -> Result<PathBuf, String> {
    let path = path.trim().trim_matches(|c| c == '"' || c == '\'');
    if path.is_empty() {
        return Err("empty path".into());
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if let Some(rest) = path.strip_prefix("$HOME/") {
        format!("{home}/{rest}")
    } else if let Some(rest) = path.strip_prefix("$LAZAR_HOME/") {
        format!("{}/{rest}", crate::lazar_home().display())
    } else {
        path.to_string()
    };
    let expanded = PathBuf::from(expanded);
    Ok(if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    })
}

fn path_and_number(rest: &str, cwd: &Path) -> Result<(PathBuf, u64), String> {
    let (path, number) = rest
        .rsplit_once(':')
        .ok_or("expected <path>:<number>")?;
    let number = number
        .trim()
        .replace('_', "")
        .parse::<u64>()
        .map_err(|_| format!("'{}' is not a whole number", number.trim()))?;
    Ok((resolve(path, cwd)?, number))
}

pub(crate) fn parse(line: &str, cwd: &Path) -> Result<Claim, String> {
    let line = clean_line(line);
    let (key, rest) = line
        .split_once('=')
        .ok_or_else(|| format!("expected key=value (keys: {KEYS})"))?;
    let rest = rest.trim();
    match key.trim() {
        "exists" => Ok(Claim::Exists(resolve(rest, cwd)?)),
        "not_exists" => Ok(Claim::NotExists(resolve(rest, cwd)?)),
        "size" => path_and_number(rest, cwd).map(|(p, n)| Claim::Size(p, n)),
        "size_at_least" => path_and_number(rest, cwd).map(|(p, n)| Claim::SizeAtLeast(p, n)),
        "size_at_most" => path_and_number(rest, cwd).map(|(p, n)| Claim::SizeAtMost(p, n)),
        "mtime_within" => path_and_number(rest, cwd).map(|(p, n)| Claim::MtimeWithin(p, n)),
        "contains" => {
            let (path, needle) = rest.split_once(':').ok_or("expected <path>:<text>")?;
            let needle = needle.trim().trim_matches(|c| c == '"' || c == '\'');
            if needle.is_empty() {
                return Err("empty contains= text".into());
            }
            if needle.chars().count() > MAX_NEEDLE_CHARS {
                return Err(format!(
                    "contains= text is over {MAX_NEEDLE_CHARS} characters; use a short, distinctive token"
                ));
            }
            Ok(Claim::Contains(resolve(path, cwd)?, needle.to_string()))
        }
        other => Err(format!("unknown claim key '{other}' (keys: {KEYS})")),
    }
}

/// Collapse runs of whitespace so a claim survives reflowed text.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn size_of(path: &Path) -> Result<u64, String> {
    let meta = fs::metadata(path).map_err(|_| format!("{} does not exist", path.display()))?;
    if meta.is_dir() {
        return Err(format!("{} is a directory", path.display()));
    }
    Ok(meta.len())
}

fn run(claim: &Claim) -> Result<String, String> {
    match claim {
        Claim::Exists(p) => {
            if p.exists() {
                Ok("exists".into())
            } else {
                Err(format!("{} does not exist", p.display()))
            }
        }
        Claim::NotExists(p) => {
            if p.exists() {
                Err(format!("{} still exists", p.display()))
            } else {
                Ok("absent".into())
            }
        }
        Claim::Size(p, n) => {
            let size = size_of(p)?;
            if size == *n { Ok(format!("{size} bytes")) } else { Err(format!("{size} bytes, not {n}")) }
        }
        Claim::SizeAtLeast(p, n) => {
            let size = size_of(p)?;
            if size >= *n { Ok(format!("{size} bytes")) } else { Err(format!("{size} bytes, under {n}")) }
        }
        Claim::SizeAtMost(p, n) => {
            let size = size_of(p)?;
            if size <= *n { Ok(format!("{size} bytes")) } else { Err(format!("{size} bytes, over {n}")) }
        }
        Claim::MtimeWithin(p, secs) => {
            let modified = fs::metadata(p)
                .and_then(|m| m.modified())
                .map_err(|_| format!("{} does not exist", p.display()))?;
            let age = SystemTime::now()
                .duration_since(modified)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if age <= *secs {
                Ok(format!("modified {age}s ago"))
            } else {
                Err(format!("modified {age}s ago, not within {secs}s"))
            }
        }
        Claim::Contains(p, needle) => {
            let size = size_of(p)?;
            if size > MAX_CONTAINS_FILE_BYTES {
                return Err(format!("{} is {size} bytes; too large to search", p.display()));
            }
            let bytes = fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            let text = String::from_utf8_lossy(&bytes);
            if text.contains(needle.as_str()) || squash(&text).contains(&squash(needle)) {
                Ok("found".into())
            } else {
                Err(format!("text not found in {}", p.display()))
            }
        }
    }
}

/// Parse and check one claim line.
pub(crate) fn check(line: &str, cwd: &Path) -> Outcome {
    let claim = clean_line(line).to_string();
    let result = parse(&claim, cwd).and_then(|parsed| run(&parsed));
    match result {
        Ok(detail) => Outcome { claim, ok: true, detail },
        Err(detail) => Outcome { claim, ok: false, detail },
    }
}

pub(crate) fn check_all(lines: &[String], cwd: &Path) -> Vec<Outcome> {
    lines.iter().map(|line| check(line, cwd)).collect()
}

/// Operator-role message for the one correction turn. Starts with `[kernel`
/// so clients can tell kernel nudges from operator text on replay.
pub(crate) fn correction_prompt(outcomes: &[Outcome]) -> String {
    let mut text = String::from(
        "[kernel verification] Your [VERIFY] claims were checked against the real filesystem and some did not hold:\n",
    );
    for outcome in outcomes.iter().filter(|o| !o.ok) {
        text.push_str(&format!("- {} — {}\n", outcome.claim, outcome.detail));
    }
    text.push_str(
        "Inspect the actual state with tools, fix what is wrong, then give your answer again. End it with a [VERIFY] block of claims that now hold, or no block if you no longer claim that outcome. Do not restate a claim you have not re-checked.",
    );
    text
}

/// Summary for an error when verification fails for good.
pub(crate) fn failure_message(outcomes: &[Outcome]) -> String {
    let failed: Vec<String> = outcomes
        .iter()
        .filter(|o| !o.ok)
        .map(|o| format!("{} ({})", o.claim, o.detail))
        .collect();
    format!(
        "grounding failed: {} of {} claim(s) did not hold after a correction turn: {}",
        failed.len(),
        outcomes.len(),
        failed.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lazar-verify-{label}-{}-{}",
            std::process::id(),
            crate::now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn no_block_asserts_nothing_and_the_last_block_wins() {
        assert_eq!(extract_block("hello, all done"), None);
        let text = "draft\n[VERIFY]\nexists=/old\n[/VERIFY]\nfinal\n[verify]\n- `exists=/new`\n* size_at_least=/new:3\n[/VERIFY\n";
        assert_eq!(
            extract_block(text),
            Some(Ok(vec!["exists=/new".into(), "size_at_least=/new:3".into()]))
        );
    }

    #[test]
    fn broken_blocks_are_failed_proofs() {
        assert!(extract_block("[VERIFY]\nexists=/x\n").unwrap().is_err());
        assert!(extract_block("[VERIFY]\n\n[/VERIFY]").unwrap().is_err());
        let many = (0..=MAX_CLAIMS).map(|i| format!("exists=/f{i}")).collect::<Vec<_>>().join("\n");
        assert!(extract_block(&format!("[VERIFY]\n{many}\n[/VERIFY]")).unwrap().is_err());
    }

    #[test]
    fn every_claim_kind_checks_real_state() {
        let dir = scratch("kinds");
        let file = dir.join("out.txt");
        fs::write(&file, "alpha\n  beta   gamma\n").unwrap();
        let f = file.display();
        let pass = [
            format!("exists={f}"),
            format!("not_exists={}/missing", dir.display()),
            format!("size={f}:21"),
            format!("size_at_least={f}:5"),
            format!("size_at_most={f}:1_000"),
            format!("contains={f}:beta gamma"),
            format!("contains={f}:\"alpha\""),
            format!("mtime_within={f}:600"),
            "exists=out.txt".to_string(),
        ];
        for claim in &pass {
            let outcome = check(claim, &dir);
            assert!(outcome.ok, "{claim}: {}", outcome.detail);
        }
        let fail = [
            format!("exists={}/missing", dir.display()),
            format!("not_exists={f}"),
            format!("size={f}:3"),
            format!("size_at_least={f}:999"),
            format!("contains={f}:delta"),
            format!("size_at_least={}:1", dir.display()),
            "bogus=1".to_string(),
            format!("contains={f}:"),
            format!("size={f}:many"),
        ];
        for claim in &fail {
            let outcome = check(claim, &dir);
            assert!(!outcome.ok, "{claim} should fail");
            assert!(!outcome.detail.is_empty());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn correction_names_only_the_failed_claims() {
        let outcomes = vec![
            Outcome { claim: "exists=/a".into(), ok: true, detail: "exists".into() },
            Outcome { claim: "exists=/b".into(), ok: false, detail: "/b does not exist".into() },
        ];
        let prompt = correction_prompt(&outcomes);
        assert!(prompt.starts_with("[kernel verification]"));
        assert!(prompt.contains("exists=/b — /b does not exist"));
        assert!(!prompt.contains("exists=/a"));
        assert!(failure_message(&outcomes).contains("1 of 2"));
    }
}
