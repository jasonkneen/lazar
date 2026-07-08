//! stall.rs — kernel-owned agent-loop stall recovery.
//!
//! OWNER DOCTRINE (Jason, 2026-07-07): the agent loop lives in the KERNEL,
//! never in a client (TUI). When the model ends a turn by *narrating intent*
//! ("I'll check the README.") instead of calling the tool, the kernel may
//! nudge it — but recovery is BOUNDED (LAZAR_STALL_NUDGES, default 1) and
//! conservative:
//!   - fires only when the final sentence announces an unfinished action
//!   - never fires when the model is asking the operator something
//!   - never fires on long responses (real work was already shown)
//!   - never unbounded — the old 20-shot "Continue" spam caused runaway
//!     "Idle. Standing by." loops and is not allowed back.

/// The continuation prompt injected on a detected stall. Deliberately asks
/// the model to either act or end cleanly — it must not manufacture work.
pub const NUDGE_PROMPT: &str = "Your turn ended incomplete: you announced something (a trailing ':' or an 'I'll …' intent) but did not deliver it. Continue now — run the bash tool if action is needed, or write the rest of the answer. If the work is already complete, or you need the operator to decide something, say so plainly instead.";

/// Longer responses almost always contain the actual work product; a stall
/// is a short announcement with nothing behind it.
const MAX_STALL_TEXT_BYTES: usize = 1200;

/// looks_stalled reports whether the turn's final text is an unfinished
/// announcement that warrants a continuation nudge. Two shapes:
///   1. Hanging colon — "Here are the patterns I use most:" then nothing.
///      A completed answer essentially never ends on a bare ':'. This is
///      MiniMax's dominant stall signature and applies at ANY length.
///   2. Trailing first-person intent — "I'll check the README." — only for
///      short responses (long ones already contain the work).
pub fn looks_stalled(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    if awaiting_operator(&lower) {
        return false;
    }
    if lower.ends_with(':') {
        return true;
    }
    if lower.len() > MAX_STALL_TEXT_BYTES {
        return false;
    }
    trailing_intent(&lower)
}

/// awaiting_operator is true when the model is asking the human something —
/// those turns must END so the human can answer. Auto-approving questions is
/// explicitly forbidden.
fn awaiting_operator(lower: &str) -> bool {
    if lower.trim_end().ends_with('?') {
        return true;
    }
    let tail = last_sentence(lower);
    const ASK_PATTERNS: &[&str] = &[
        "should i",
        "should we",
        "shall i",
        "shall we",
        "do you want",
        "would you like",
        "may i",
        "let me know",
        "what do you want",
        "do you prefer",
        "your call",
        "up to you",
    ];
    ASK_PATTERNS.iter().any(|p| tail.contains(p))
}

/// trailing_intent is true when the LAST sentence announces an action the
/// model has not performed ("i'll check…", "let me run…").
///
/// Phrases are checked as whole-token prefixes (start of sentence or after
/// whitespace) so bare substrings like "now i" / "next i" do not fire on
/// "Now it works" / "Next item is complete".
fn trailing_intent(lower: &str) -> bool {
    let tail = last_sentence(lower)
        .trim_end_matches([':', ';'])
        .trim()
        .to_string();
    if tail.is_empty() {
        return false;
    }
    const INTENT_PREFIXES: &[&str] = &[
        "i'll ",
        "i will ",
        "let me ",
        "i'm going to ",
        "im going to ",
        "i am going to ",
        "checking now",
        "running now",
        "reading now",
        "on it",
        "one moment",
        "give me a moment",
        "do it now",
        "do that now",
        "now i'll ",
        "now i will ",
        "now let me ",
        "next i'll ",
        "next i will ",
        "next let me ",
    ];
    INTENT_PREFIXES.iter().any(|p| phrase_at_boundary(&tail, p))
}

/// True if `phrase` appears at the start of `s` or immediately after a space.
fn phrase_at_boundary(s: &str, phrase: &str) -> bool {
    if s.starts_with(phrase) {
        return true;
    }
    let mut search_from = 0;
    while let Some(rel) = s[search_from..].find(phrase) {
        let i = search_from + rel;
        if i > 0 && s.as_bytes()[i - 1] == b' ' {
            return true;
        }
        search_from = i + 1;
        if search_from >= s.len() {
            break;
        }
    }
    false
}

/// last_sentence returns the trimmed final sentence, splitting on . ! ? and
/// newlines. Returns the whole string when no terminator precedes it.
fn last_sentence(s: &str) -> String {
    let trimmed = s.trim_end();
    let mut start = 0;
    for (i, c) in trimmed.char_indices() {
        if matches!(c, '.' | '!' | '?' | '\n') {
            // sentence boundary: candidate start is after this char
            let after = i + c.len_utf8();
            if after < trimmed.len() {
                start = after;
            }
        }
    }
    trimmed[start..].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nudges_trailing_intent_announcement() {
        assert!(looks_stalled("I'll check the README."));
        assert!(looks_stalled("Found the issue. Let me run the tests:"));
        assert!(looks_stalled("I'm going to restart the heartbeat now"));
        assert!(looks_stalled("Now I'll check the README."));
        assert!(looks_stalled("Next I'll open the file."));
    }

    #[test]
    fn nudges_hanging_colon_announcements() {
        // Regression: MiniMax-M3 ended turns exactly like this (2026-07-07)
        // and the phrase-based heuristic missed both.
        assert!(looks_stalled(
            "Sure. `rg` (ripgrep) is the right tool for searching — way faster \
             than `grep -r` and respects `.gitignore` by default. Here are the \
             patterns I use most:"
        ));
        assert!(looks_stalled(
            "Practical examples — running them for real so you see actual output:"
        ));
        // Hanging colon applies regardless of response length.
        let long = format!("{} And here is the full list:", "context ".repeat(300));
        assert!(looks_stalled(&long));
    }

    #[test]
    fn never_nudges_questions_to_operator() {
        assert!(!looks_stalled("Should I proceed with the migration?"));
        assert!(!looks_stalled("what do you actually want me to do?"));
        assert!(!looks_stalled(
            "Two options: keep or delete. Let me know which you prefer."
        ));
        // Menu-style ask ending in a colon is still a question, not a stall.
        assert!(!looks_stalled("Pick whichever you want — which do you prefer:"));
    }

    #[test]
    fn never_nudges_idle_standing_by() {
        // Regression: the 2026-07-07 runaway loop was the model answering
        // every blind "Continue" with this. It must never trigger recovery.
        assert!(!looks_stalled("Idle. Standing by."));
        assert!(!looks_stalled("Standing by."));
    }

    #[test]
    fn never_nudges_completed_or_empty_turns() {
        assert!(!looks_stalled(""));
        assert!(!looks_stalled("Done. All 63 tests pass."));
        let long = format!("{} I'll continue.", "x".repeat(1300));
        assert!(!looks_stalled(&long));
    }

    #[test]
    fn never_nudges_substring_false_positives() {
        // Old bare "now i" / "next i" / "going to " substrings fired mid-word.
        assert!(!looks_stalled("Now it works fine."));
        assert!(!looks_stalled("Next item is complete."));
        assert!(!looks_stalled("Done. Going to the office tomorrow."));
        assert!(!looks_stalled("Now is the time to rest."));
        // First-person "I'm going to …" remains a stall (agent domain).
    }

    #[test]
    fn intent_must_be_in_final_sentence() {
        // Announced, then actually reported results — no nudge.
        assert!(!looks_stalled(
            "Let me check the log. The log shows three errors, all fixed."
        ));
    }

    #[test]
    fn last_sentence_handles_multibyte_and_no_terminator() {
        assert_eq!(last_sentence("done — ok. i'll fix it"), "i'll fix it");
        assert_eq!(last_sentence("no terminator"), "no terminator");
    }
}
