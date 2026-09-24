//! Turn-loop safeguards: the model must keep the operator informed, and must
//! not burn rounds re-running a command whose result cannot change.

use serde_json::Value;
use std::collections::VecDeque;

/// Tool rounds in a row with no visible text before the kernel asks for an update.
pub(crate) const SILENT_ROUNDS: u32 = 4;
/// Recent commands kept for repeat detection.
const WINDOW: usize = 6;
/// Identical commands in the window that trigger a warning.
pub(crate) const REPEAT_WARN: usize = 3;
/// Identical commands in the window that end the turn.
pub(crate) const REPEAT_ABORT: usize = 5;

pub(crate) const SILENCE_NOTE: &str = "[kernel] You have run several tool calls without telling the operator anything. Before your next tool call, write one or two plain sentences: what you are doing right now and what you have found so far.";

pub(crate) enum Verdict {
    Ok,
    Nudge(String),
    Abort(String),
}

#[derive(Default)]
pub(crate) struct Guard {
    silent: u32,
    recent: VecDeque<String>,
}

impl Guard {
    /// Observe one assistant response; returns what the kernel must do.
    pub(crate) fn observe(&mut self, content: &[Value]) -> Verdict {
        let spoke = content
            .iter()
            .any(|block| block["type"] == "text" && block["text"].as_str().is_some_and(|t| !t.trim().is_empty()));
        let commands: Vec<String> = content
            .iter()
            .filter(|block| block["type"] == "tool_use")
            .map(|block| block["input"].to_string())
            .collect();
        if commands.is_empty() {
            self.silent = 0;
            return Verdict::Ok;
        }
        self.silent = if spoke { 0 } else { self.silent + 1 };

        let mut worst = (0, String::new());
        for command in commands {
            self.recent.push_back(command.clone());
            if self.recent.len() > WINDOW {
                self.recent.pop_front();
            }
            let count = self.recent.iter().filter(|c| **c == command).count();
            if count > worst.0 {
                worst = (count, command);
            }
        }
        let (count, command) = worst;
        let shown: String = command.chars().take(200).collect();
        if count >= REPEAT_ABORT {
            return Verdict::Abort(format!(
                "stopped: the same tool call ran {count} times in the last {WINDOW} calls without progress: {shown}"
            ));
        }
        let mut notes = Vec::new();
        if count >= REPEAT_WARN {
            notes.push(format!(
                "[kernel] You have run this exact tool call {count} times in your last {WINDOW} calls: {shown}\nIts result will not change. Do not run it again. Tell the operator what you are trying to find and what is blocking you, then take a different step."
            ));
        }
        if self.silent >= SILENT_ROUNDS {
            self.silent = 0;
            notes.push(SILENCE_NOTE.to_string());
        }
        if notes.is_empty() { Verdict::Ok } else { Verdict::Nudge(notes.join("\n\n")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(cmd: &str, text: &str) -> Vec<Value> {
        vec![
            json!({"type":"text","text":text}),
            json!({"type":"tool_use","id":"x","name":"bash","input":{"command":cmd}}),
        ]
    }

    #[test]
    fn silent_rounds_trigger_an_update_request() {
        let mut guard = Guard::default();
        for n in 0..SILENT_ROUNDS - 1 {
            assert!(matches!(guard.observe(&call(&format!("ls {n}"), "\n\n\n")), Verdict::Ok));
        }
        let Verdict::Nudge(note) = guard.observe(&call("ls last", "   ")) else { panic!("expected nudge") };
        assert!(note.contains("what you are doing"));
    }

    #[test]
    fn speaking_resets_the_silence_count() {
        let mut guard = Guard::default();
        for n in 0..10 {
            let text = if n % 3 == 0 { "checking the parser" } else { "" };
            assert!(matches!(guard.observe(&call(&format!("ls {n}"), text)), Verdict::Ok));
        }
    }

    #[test]
    fn repeated_commands_warn_then_abort() {
        let mut guard = Guard::default();
        let cmd = r#"grep -n "models" main.rs | head -60"#;
        assert!(matches!(guard.observe(&call(cmd, "looking")), Verdict::Ok));
        assert!(matches!(guard.observe(&call(cmd, "looking")), Verdict::Ok));
        let Verdict::Nudge(note) = guard.observe(&call(cmd, "looking")) else { panic!("expected warning") };
        assert!(note.contains("3 times"));
        assert!(matches!(guard.observe(&call(cmd, "looking")), Verdict::Nudge(_)));
        assert!(matches!(guard.observe(&call(cmd, "looking")), Verdict::Abort(_)));
    }
}
