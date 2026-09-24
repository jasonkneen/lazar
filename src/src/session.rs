//! lazar session continuity — persistent multi-turn conversations.
//!
//! When `lazar -p ... --session <id>` is invoked:
//!   1. If `logs/sessions/<id>.jsonl` exists, prior turns are loaded and
//!      prepended to the message array. The agent sees actual conversation
//!      history, not just ambient state.
//!   2. New events (user prompt, assistant turns, tool results) are
//!      appended to that kernel-owned session log AS WELL AS to stream.jsonl.
//!
//! This fixes the multi-turn referential bug: "yes" / "do that" / "fine"
//! now resolve against the actual prior turn instead of being inferred
//! from recent tool output.
//!
//! Session id format: alphanumeric + dash + underscore + dot, max 64 chars.
//! No path traversal — `..`, `/`, leading `.` rejected.

use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use crate::{core::context, lazar_home, now_millis};

/// Replayed history budget. The caller passes one derived from the model's
/// context window (`context::Window::history`); LAZAR_SESSION_HISTORY_MAX_BYTES
/// overrides it in either direction. Measured on a 117-message session against
/// Kimi K3: at 135 KB the model called its tool ~half the time and narrated the
/// rest — set the override lower for models that degrade on long replays.

/// Maximum length of a session id.
const MAX_SESSION_ID_LEN: usize = 64;

/// Validate a session id. Returns Ok(()) if safe to use as a filename,
/// Err with reason otherwise.
pub fn validate_session_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("session id is empty".into());
    }
    if id.len() > MAX_SESSION_ID_LEN {
        return Err(format!(
            "session id too long ({} > {} chars)",
            id.len(),
            MAX_SESSION_ID_LEN
        ));
    }
    if id.starts_with('.') {
        return Err("session id cannot start with '.'".into());
    }
    for c in id.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        if !ok {
            return Err(format!(
                "session id contains invalid character {:?} (allowed: a-z A-Z 0-9 - _ .)",
                c
            ));
        }
    }
    if id.contains("..") {
        return Err("session id cannot contain '..'".into());
    }
    Ok(())
}

/// Path to the trusted, kernel-owned session log for this id. The sandbox
/// profile must not grant tools write access to this tree. Caller is responsible for
/// validating the id first.
pub fn session_log_path(id: &str) -> PathBuf {
    lazar_home()
        .join("user")
        .join("logs")
        .join("sessions")
        .join(format!("{id}.jsonl"))
}

pub fn persist_metadata(id: &str, cwd: &Path, dirs: &[PathBuf]) -> Result<(), String> {
    validate_session_id(id)?;
    let path = session_log_path(id).with_extension("meta.json");
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&tmp, json!({"cwd": cwd, "additional_dirs": dirs}).to_string()).map_err(|e| e.to_string())?;
    fs::rename(tmp, path).map_err(|e| e.to_string())
}

pub fn load_metadata(id: &str) -> Option<(PathBuf, Vec<PathBuf>)> {
    validate_session_id(id).ok()?;
    let value: Value = serde_json::from_slice(&fs::read(session_log_path(id).with_extension("meta.json")).ok()?).ok()?;
    Some((PathBuf::from(value["cwd"].as_str()?), serde_json::from_value(value["additional_dirs"].clone()).ok()?))
}

/// Find the most recently modified session log under
/// `$LAZAR_HOME/logs/sessions/`. Returns the session id (filename
/// stem), or None if no session logs exist or the directory is
/// missing. Used by `--resume`.
pub fn find_newest_session() -> Option<String> {
    let dir = crate::user_dir("logs").join("sessions");
    let entries = fs::read_dir(&dir).ok()?;

    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "jsonl" {
            continue;
        }
        // Re-validate the id: skip anything that wouldn't pass our
        // own validation, even if it somehow ended up on disk.
        if validate_session_id(&stem).is_err() {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        match &best {
            Some((best_t, _)) if *best_t >= mtime => {}
            _ => best = Some((mtime, stem)),
        }
    }
    best.map(|(_, id)| id)
}

/// One session log discovered under `$LAZAR_HOME/logs/sessions/`.
#[derive(Debug, Clone)]
pub struct SessionListEntry {
    pub id: String,
    pub modified: Option<std::time::SystemTime>,
}

/// List kernel-owned session logs, newest first.
pub fn list_sessions() -> Vec<SessionListEntry> {
    let dir = crate::user_dir("logs").join("sessions");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "jsonl" {
            continue;
        }
        if validate_session_id(&stem).is_err() {
            continue;
        }
        let modified = entry.metadata().and_then(|m| m.modified()).ok();
        out.push(SessionListEntry {
            id: stem,
            modified,
        });
    }
    out.sort_by_key(|entry| std::cmp::Reverse(entry.modified));
    out
}

pub fn session_exists(id: &str) -> bool {
    validate_session_id(id).is_ok() && session_log_path(id).is_file()
}

pub fn delete_session(id: &str) -> Result<(), String> {
    validate_session_id(id)?;
    let _ = fs::remove_file(session_log_path(id).with_extension("meta.json"));
    match fs::remove_file(session_log_path(id)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to delete session {id}: {err}")),
    }
}

/// Read prior turns from a session log and convert them into the
/// Anthropic Messages API format. Returns an empty Vec if the log
/// doesn't exist (first turn).
///
/// History preserves replayable tool/result structure within the context limits: an assistant turn keeps its text and its
/// `tool_use` blocks, and the results come back as real `tool_result`
/// blocks in the following user message. The model sees what it did, not a
/// paraphrase of it.
///
/// This replaced a loader that stripped every tool call and result and put a
/// kernel-written "[KERNEL TOOL EVIDENCE] shell command (3 arguments) exit 0"
/// summary in the USER role instead. Fed back a long session, the model saw a
/// transcript in which its own turns were bare "Let me check…" narration and
/// the tool happened in the operator's turn — and learned, in context, to
/// narrate and stop. Two unrelated models produced the identical failure on
/// the same session; a fresh `lazar -p` with no history did not.
///
/// A `tool_use` whose result was never logged (interrupted turn) gets an
/// explicit outcome-unknown `tool_result`. The command is not run again.
/// Truncation only cuts at an operator message, so pairing stays intact.
///
/// Tool result bodies are capped per result (`HISTORY_TOOL_RESULT_MAX_BYTES`)
/// and the whole history under LAZAR_SESSION_HISTORY_MAX_BYTES. Thinking
/// blocks are dropped (they need provider signatures to be replayed).
///
/// The returned messages are guaranteed to:
/// - Start with an operator (plain-text user) message.
/// - Alternate user/assistant with every tool_use answered.
/// - Stay under the compiled session-history ceiling.
///
/// `Err` means the log contains a conversation that cannot be continued
/// (truncated past a safe boundary, or otherwise unreplayable). That error
/// is not a conversation item: callers must not replay it as a user or
/// assistant turn. A missing or empty log is `Ok` and empty.
pub fn load_messages_checked(id: &str, history_cap: usize) -> Result<Vec<Value>, String> {
    let path = session_log_path(id);
    let (messages, repairs) = load_history(&path, resolve_history_cap(history_cap));
    for repair in repairs {
        append_session(id, repair)?;
    }
    if messages.is_empty() && log_contains_conversation(&path) {
        return Err("session context limit or invalid history prevents a valid continuation; start a new session with a concise task summary".into());
    }
    Ok(messages)
}

#[cfg(test)]
fn load_messages_checked_from_path(path: &Path) -> Result<Vec<Value>, String> {
    let (messages, _) = load_history(path, resolve_history_cap(TEST_HISTORY_CAP));
    if messages.is_empty() && log_contains_conversation(path) {
        return Err("session context limit or invalid history prevents a valid continuation; start a new session with a concise task summary".into());
    }
    Ok(messages)
}

/// True when the log holds at least one operator or assistant event.
/// A line that is not valid UTF-8 is skipped; it does not hide later events.
fn log_contains_conversation(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    BufReader::new(file).lines().any(|line| {
        line.ok()
            .and_then(|line| serde_json::from_str::<Value>(&line).ok())
            .is_some_and(|event| matches!(event["kind"].as_str(), Some("user" | "assistant")))
    })
}

/// Largest tool output replayed per result in the most recent rounds. Full
/// outputs live in the log; history needs enough to remember what happened,
/// not the whole file.
const HISTORY_TOOL_RESULT_MAX_BYTES: usize = 2_000;

/// Path-parameterized loader backing `load_messages_checked`. Split out so tests
/// can exercise the tail-bounded read against a temp file directly,
/// without mutating the process-global LAZAR_HOME env var (which races
/// with other test modules under the multithreaded test runner).
#[cfg(test)]
fn load_messages_from_path(path: &Path) -> Vec<Value> {
    load_history(path, resolve_history_cap(TEST_HISTORY_CAP)).0
}

fn load_history(path: &Path, cap: usize) -> (Vec<Value>, Vec<Value>) {
    if !path.is_file() {
        return (vec![], vec![]);
    }
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (vec![], vec![]), // no prior turns
    };

    // Session logs are never rotated and mirror every stream event
    // (including tool_result output up to LAZAR_TOOL_OUTPUT_MAX_BYTES),
    // so a long-lived session can grow to megabytes even though only a
    // small, recent slice of it ever survives into loaded history. Avoid
    // parsing the whole file: read a bounded tail with headroom (4x the
    // resolved cap — discarded lines still count against the read budget,
    // so headroom keeps enough real turns in view to fill the cap).
    let budget = (cap as u64).saturating_mul(4);
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);

    let mut reader = BufReader::new(file);
    if file_len > budget {
        let offset = file_len.saturating_sub(budget);
        if reader.seek(SeekFrom::Start(offset)).is_ok() {
            // We've very likely landed mid-line; discard everything up
            // to and including the first newline so parsing starts on a
            // clean record boundary instead of a truncated JSON blob.
            let mut discard = String::new();
            let _ = reader.read_line(&mut discard);
        }
    }

    let mut history = History::default();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let event: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match event.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
            "user" => {
                if let Some(text) = event.get("model_content").and_then(Value::as_str) {
                    history.user_text(text);
                } else if let Some(text) = event.get("content").and_then(Value::as_str) {
                    history.user_text(text);
                }
            }
            "assistant" => history.assistant(event.get("content")),
            "tool_result" => history.tool_result(&event),
            "continuation" => history.user_text(crate::core::compact::CONTINUE_PROMPT),
            "steer" => {
                if let Some(text) = event.get("content").and_then(Value::as_str) {
                    history.user_text(text);
                }
            }
            "verify_correction" => {
                if let Some(text) = event.get("content").and_then(Value::as_str) {
                    history.user_text(text);
                }
            }
            _ => {}
        }
    }

    let (messages, repairs) = history.finish();
    (truncate_to_cap(messages, cap), repairs)
}

const INTERRUPTED_TOOL_RESULT: &str = "[no result recorded — the turn was interrupted. The command's side effect may already have happened; do not retry it unless the user asks.]";

/// Builds a valid Messages-API conversation from raw log events.
#[derive(Default)]
struct History {
    messages: Vec<Value>,
    /// Tool-use ids in the latest assistant turn that have not seen a result.
    /// IDs are replayed exactly as the provider saw them so resumed prefixes
    /// are retained when present; truncation and repair can change the prefix.
    pending: Vec<(String, String)>,
    /// Outcome-unknown results synthesized for tool calls the log never closed.
    repairs: Vec<Value>,
    next_id: usize,
}

impl History {
    fn user_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let text = context::truncate_item(text);
        self.close_pending();
        if let Some(last) = self.messages.last_mut() {
            if last["role"] == "user" {
                // Two operator prompts with no assistant turn between them
                // (an errored invocation, say) fold into one message…
                if let Some(existing) = last["content"].as_str() {
                    last["content"] = json!(context::truncate_item(&format!(
                        "{existing}\n\n{text}"
                    )));
                    return;
                }
                // …and a prompt typed straight after an interrupted tool
                // round rides in the same user message as those results,
                // which is the canonical Messages-API shape for it.
                if let Some(arr) = last["content"].as_array_mut() {
                    arr.push(json!({"type": "text", "text": text}));
                    return;
                }
            }
        }
        self.messages.push(json!({"role": "user", "content": text}));
    }

    fn assistant(&mut self, content: Option<&Value>) {
        let Some(blocks) = content.and_then(Value::as_array) else {
            // Plain-string assistant content (older logs).
            if let Some(text) = content.and_then(Value::as_str).filter(|t| !t.is_empty()) {
                self.assistant(Some(&json!([{"type": "text", "text": text}])));
            }
            return;
        };
        self.close_pending();
        let mut kept: Vec<Value> = Vec::new();
        let mut new_pending: Vec<(String, String)> = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            kept.push(json!({
                                "type": "text",
                                "text": context::truncate_item(text),
                            }));
                        }
                    }
                }
                Some("tool_use") => {
                    let original = block.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                    let id = if original.is_empty() {
                        self.next_id += 1;
                        format!("replayed_tool_{}", self.next_id)
                    } else {
                        original.clone()
                    };
                    if block.get("input").is_some_and(|input| input.to_string().len() > context::MAX_CONTEXT_ITEM_BYTES) {
                        kept.push(json!({"type": "text", "text": "[Historical tool input omitted: exceeded context limit. Its result is omitted with it.]"}));
                        continue;
                    }
                    kept.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": block.get("name").cloned().unwrap_or(json!("bash")),
                        "input": block.get("input").cloned().unwrap_or(json!({})),
                    }));
                    new_pending.push((original, id));
                }
                // thinking / redacted_thinking / anything else: not replayable
                // without the provider's own signature.
                _ => {}
            }
        }
        if kept.is_empty() {
            return;
        }
        // History must open with an operator message; an assistant turn with
        // nothing before it (tail-bounded read landed mid-turn) is dropped.
        let Some(last) = self.messages.last_mut() else { return };
        if last["role"] == "assistant" {
            // Consecutive assistant turns with no result between them: the
            // earlier one had no tool calls (close_pending would have
            // answered them), so extend it.
            if let Some(arr) = last["content"].as_array_mut() {
                arr.extend(kept);
            }
        } else {
            self.messages.push(json!({"role": "assistant", "content": kept}));
        }
        self.pending = new_pending;
    }

    fn tool_result(&mut self, event: &Value) {
        let original = event.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
        let Some(pos) = self.pending.iter().position(|(o, _)| o == original) else {
            return; // orphan: its tool_use is not in the replayed window
        };
        let (_, id) = self.pending.remove(pos);
        let content = bounded_result(
            event.get("content").and_then(Value::as_str).unwrap_or(""),
            event.get("exit_code").and_then(Value::as_i64),
        );
        let exit_code = event.get("exit_code").cloned().unwrap_or(Value::Null);
        let is_error = event["is_error"] == true || exit_code.as_i64().is_some_and(|code| code != 0);
        self.push_result(json!({"type": "tool_result", "tool_use_id": id, "content": content, "exit_code": exit_code, "is_error": is_error}));
    }

    /// Answer every tool_use still waiting so the conversation stays valid:
    /// the turn was interrupted before the kernel logged a result.
    fn close_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for (_, id) in pending {
            self.push_result(json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": INTERRUPTED_TOOL_RESULT,
                "is_error": true,
            }));
            self.repairs.push(json!({
                "kind": "tool_result",
                "tool_use_id": id,
                "content": INTERRUPTED_TOOL_RESULT,
                "exit_code": -1,
                "is_error": true,
            }));
        }
    }

    fn push_result(&mut self, block: Value) {
        if let Some(last) = self.messages.last_mut() {
            if last["role"] == "user" && last["content"].is_array() {
                if let Some(arr) = last["content"].as_array_mut() {
                    arr.push(block);
                }
                return;
            }
        }
        self.messages.push(json!({"role": "user", "content": [block]}));
    }

    fn finish(mut self) -> (Vec<Value>, Vec<Value>) {
        self.close_pending();
        (self.messages, self.repairs)
    }
}

/// Cap one replayed tool output. The kernel's result text already ends in
/// `[exit N]`; keep that marker visible when the body is cut.
fn bounded_result(content: &str, exit_code: Option<i64>) -> String {
    bounded_result_to(content, exit_code, HISTORY_TOOL_RESULT_MAX_BYTES)
}

fn bounded_result_to(content: &str, exit_code: Option<i64>, cap: usize) -> String {
    if content.len() <= cap {
        return content.to_string();
    }
    let head = crate::text::head_bytes(content, cap);
    let omitted = content.len() - head.len();
    let exit = exit_code.map(|c| format!("\n[exit {c}]")).unwrap_or_default();
    format!("{head}\n[… {omitted} bytes omitted from replayed history]{exit}")
}

/// True for a message the operator typed (as opposed to a tool_result
/// carrier). Only these are valid places to start a truncated history.
fn is_operator_message(message: &Value) -> bool {
    message["role"] == "user" && message["content"].is_string()
}

/// Resolve the session-history byte cap: LAZAR_SESSION_HISTORY_MAX_BYTES
/// when set to a positive value, else the window-derived `default`.
fn resolve_history_cap(default: usize) -> usize {
    std::env::var("LAZAR_SESSION_HISTORY_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n: &usize| *n > 0)
        .unwrap_or(default)
}

/// History cap tests use when they do not set the env override.
#[cfg(test)]
const TEST_HISTORY_CAP: usize = 32_000;

#[cfg(test)]
fn truncate_to_byte_cap(messages: Vec<Value>) -> Vec<Value> {
    truncate_to_cap(messages, resolve_history_cap(TEST_HISTORY_CAP))
}

/// Drop oldest messages until the remaining set fits under the byte cap.
/// Always keeps the conversation valid: starts with an operator message, so
/// every replayed tool_result still follows the assistant turn that owns it.
///
/// Each message's serialized size is computed exactly once and summed;
/// dropping from the front updates a running total instead of
/// re-serializing the whole array per removal, so this is linear in the
/// number of messages rather than O(dropped x total).
fn truncate_to_cap(mut messages: Vec<Value>, cap: usize) -> Vec<Value> {
    // Drop oldest operator rounds until the remainder fits. A round starts at
    // a plain user message. Tool-result carriers are not round starts: dropping
    // those without their tool_use is what emptied a real session and made the
    // next prompt refuse.
    loop {
        let starts: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| is_operator_message(message))
            .map(|(index, _)| index)
            .collect();
        if starts.is_empty() {
            return Vec::new();
        }
        if starts[0] > 0 {
            messages.drain(0..starts[0]);
            continue;
        }
        if json_len(&messages) <= cap || starts.len() < 2 {
            break;
        }
        messages.drain(0..starts[1]);
    }
    if json_len(&messages) > cap {
        if let Some(user) = messages.iter().find(|message| is_operator_message(message)).cloned() {
            messages = vec![user];
        }
        if json_len(&messages) > cap {
            if let Some(text) = messages.first().and_then(|message| message["content"].as_str()) {
                let keep: String = text.chars().take(1_500).collect();
                messages[0]["content"] = json!(keep);
            }
        }
    }
    messages
}

fn json_len(messages: &[Value]) -> usize {
    serde_json::to_string(messages).map(|encoded| encoded.len()).unwrap_or(0)
}

/// Full-array serialized size — kept for test assertions only. Production
/// code uses `message_size` (per-message) to avoid re-serializing the
/// whole array on every drop; see `truncate_to_byte_cap`.
#[cfg(test)]
fn serialized_size(messages: &[Value]) -> usize {
    serde_json::to_string(messages)
        .map(|s| s.len())
        .unwrap_or(0)
}

/// Append a single event to the session log. Created lazily on first
/// write. Same JSONL format as stream.jsonl.
///
/// The kernel's `append_stream` calls this when a session id is set
/// for the current invocation.
pub fn append_session(id: &str, mut event: Value) -> Result<(), String> {
    let path = session_log_path(id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("failed to append session log {}: {e}", path.display()))?;
    }

    if let Some(obj) = event.as_object_mut() {
        obj.entry("ts_ms".to_string())
            .or_insert_with(|| json!(now_millis()));
    }

    let line = serde_json::to_string(&event)
        .map_err(|e| format!("failed to append session log {}: {e}", path.display()))?;

    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }

    let mut file = opts.open(&path).map_err(|e| {
        format!("failed to append session log {}: {e}", path.display())
    })?;
    writeln!(file, "{line}").map_err(|e| format!("failed to append session log {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that read or mutate the process-global
    /// LAZAR_SESSION_HISTORY_MAX_BYTES env var. Rust runs tests on
    /// multiple threads by default, so unguarded set_var/remove_var races
    /// between tests in this module. Lock poison-tolerantly: a panicking
    /// test must not cascade-fail the others.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn replay_preserves_failed_exit_code_separately_from_output() {
        let messages = replay(&[
            json!({"kind":"user","content":"run a command"}),
            json!({"kind":"assistant","content":[{"type":"tool_use","id":"one","name":"bash","input":{"command":"exit 23"}}]}),
            json!({"kind":"tool_result","tool_use_id":"one","content":"rewritten by a hook","exit_code":23}),
        ]);
        assert_eq!(messages[2]["content"][0]["exit_code"], 23);
        assert_eq!(messages[2]["content"][0]["is_error"], true);
    }

    #[test]
    fn oversized_replay_input_omits_its_result_as_a_pair() {
        let messages = replay(&[
            json!({"kind":"user","content":"run a command"}),
            json!({"kind":"assistant","content":[{"type":"tool_use","id":"one","name":"bash","input":{"command":"x".repeat(context::MAX_CONTEXT_ITEM_BYTES)}}]}),
            json!({"kind":"tool_result","tool_use_id":"one","content":"orphan","exit_code":0}),
        ]);
        assert!(!serde_json::to_string(&messages).unwrap().contains("orphan"));
        assert!(!messages.iter().any(|m| m["content"].as_array().is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_use"))));
    }

    #[test]
    fn validate_session_id_accepts_simple_ids() {
        assert!(validate_session_id("abc").is_ok());
        assert!(validate_session_id("session-123").is_ok());
        assert!(validate_session_id("tui_chat.456").is_ok());
        assert!(validate_session_id("AbC-DeF_123").is_ok());
    }

    #[test]
    fn validate_session_id_rejects_bad_chars() {
        assert!(validate_session_id("with/slash").is_err());
        assert!(validate_session_id("with space").is_err());
        assert!(validate_session_id("with\\backslash").is_err());
        assert!(validate_session_id("with$dollar").is_err());
        assert!(validate_session_id("").is_err());
    }

    #[test]
    fn validate_session_id_rejects_path_traversal() {
        assert!(validate_session_id("..").is_err());
        assert!(validate_session_id("..foo").is_err());
        assert!(validate_session_id("foo..").is_err());
        assert!(validate_session_id(".hidden").is_err());
    }

    #[test]
    fn validate_session_id_rejects_too_long() {
        let too_long: String = "a".repeat(65);
        assert!(validate_session_id(&too_long).is_err());
        let just_right: String = "a".repeat(64);
        assert!(validate_session_id(&just_right).is_ok());
    }

    #[test]
    fn truncate_to_byte_cap_keeps_recent_messages() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LAZAR_SESSION_HISTORY_MAX_BYTES", "200");
        let messages = vec![
            json!({"role":"user","content":"old user 1"}),
            json!({"role":"assistant","content":"old asst 1"}),
            json!({"role":"user","content":"recent user"}),
            json!({"role":"assistant","content":"recent asst"}),
        ];
        let result = truncate_to_byte_cap(messages);
        // Should drop oldest until under cap
        assert!(serialized_size(&result) <= 200);
        // First remaining message must be user-role
        if !result.is_empty() {
            assert_eq!(
                result[0].get("role").and_then(|v| v.as_str()),
                Some("user")
            );
        }
        std::env::remove_var("LAZAR_SESSION_HISTORY_MAX_BYTES");
    }

    #[test]
    fn truncate_to_byte_cap_strips_leading_assistant_after_drop() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LAZAR_SESSION_HISTORY_MAX_BYTES", "100");
        // Three messages, total > 100 bytes. After dropping head,
        // an assistant should be next — we should drop it too.
        let messages = vec![
            json!({"role":"user","content":"x".repeat(60)}),
            json!({"role":"assistant","content":"y".repeat(60)}),
            json!({"role":"user","content":"z"}),
        ];
        let result = truncate_to_byte_cap(messages);
        if !result.is_empty() {
            assert_eq!(
                result[0].get("role").and_then(|v| v.as_str()),
                Some("user")
            );
        }
        std::env::remove_var("LAZAR_SESSION_HISTORY_MAX_BYTES");
    }

    /// Feed raw log events through the loader exactly as `load_messages_from_path` does.
    fn replay(events: &[Value]) -> Vec<Value> {
        let mut h = History::default();
        for e in events {
            match e["kind"].as_str().unwrap_or("") {
                "user" => h.user_text(
                    e.get("model_content")
                        .and_then(Value::as_str)
                        .or_else(|| e["content"].as_str())
                        .unwrap_or(""),
                ),
                "assistant" => h.assistant(e.get("content")),
                "tool_result" => h.tool_result(e),
                _ => {}
            }
        }
        h.finish().0
    }

    #[test]
    fn history_replays_tool_calls_and_results_faithfully() {
        // One real turn as the kernel logs it: prompt, assistant text +
        // tool_use (Anthropic-shaped id), the result, the final answer.
        let messages = replay(&[
            json!({"kind":"user","content":"how big is the log?"}),
            json!({"kind":"assistant","content":[
                {"type":"text","text":"Let me check."},
                {"type":"tool_use","id":"toolu_01ABC","name":"bash","input":{"command":"wc -c logs/stream.jsonl"}}
            ]}),
            json!({"kind":"tool_result","tool_use_id":"toolu_01ABC","content":"539282 logs/stream.jsonl\n\n[exit 0]","exit_code":0,
                   "command":"wc -c logs/stream.jsonl"}),
            json!({"kind":"assistant","content":[{"type":"text","text":"539 KB."}]}),
        ]);
        assert_eq!(messages.len(), 4, "{messages:#?}");
        assert_eq!(messages[0], json!({"role":"user","content":"how big is the log?"}));
        // The model sees that it CALLED the tool, not a paraphrase in the user's voice.
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["text"], "Let me check.");
        assert_eq!(messages[1]["content"][1]["type"], "tool_use");
        assert_eq!(messages[1]["content"][1]["input"]["command"], "wc -c logs/stream.jsonl");
        // …and the real result, paired by id.
        let id = messages[1]["content"][1]["id"].as_str().unwrap();
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], id);
        assert_eq!(messages[2]["content"][0]["content"], "539282 logs/stream.jsonl\n\n[exit 0]");
        assert_eq!(messages[3]["content"][0]["text"], "539 KB.");
        // Nothing is narrated on the model's behalf.
        let dump = serde_json::to_string(&messages).unwrap();
        assert!(!dump.contains("KERNEL TOOL EVIDENCE"));
        assert!(!dump.contains("shell command ("));
    }

    #[test]
    fn tool_ids_replay_exactly_for_stable_resumed_prefixes() {
        let messages = replay(&[
            json!({"kind":"user","content":"a"}),
            json!({"kind":"assistant","content":[{"type":"tool_use","id":"toolu_anthropic","name":"bash","input":{"command":"true"}}]}),
            json!({"kind":"tool_result","tool_use_id":"toolu_anthropic","content":"[exit 0]","exit_code":0}),
            json!({"kind":"assistant","content":[{"type":"text","text":"ok"}]}),
            json!({"kind":"user","content":"b"}),
            json!({"kind":"assistant","content":[{"type":"tool_use","id":"call_1597ea5287d7","name":"bash","input":{"command":"false"}}]}),
            json!({"kind":"tool_result","tool_use_id":"call_1597ea5287d7","content":"[exit 1]","exit_code":1}),
            json!({"kind":"assistant","content":[{"type":"text","text":"failed"}]}),
        ]);
        let ids: Vec<&str> = messages
            .iter()
            .filter(|m| m["role"] == "assistant")
            .flat_map(|m| m["content"].as_array().unwrap())
            .filter(|b| b["type"] == "tool_use")
            .map(|b| b["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["toolu_anthropic", "call_1597ea5287d7"]);
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "toolu_anthropic");
        assert_eq!(messages[6]["content"][0]["tool_use_id"], "call_1597ea5287d7");
    }

    #[test]
    fn interrupted_tool_use_gets_an_explicit_result_and_orphans_are_dropped() {
        let messages = replay(&[
            json!({"kind":"user","content":"go"}),
            // Two calls, only the first ever produced a result (cancelled turn).
            json!({"kind":"assistant","content":[
                {"type":"tool_use","id":"t1","name":"bash","input":{"command":"one"}},
                {"type":"tool_use","id":"t2","name":"bash","input":{"command":"two"}}
            ]}),
            json!({"kind":"tool_result","tool_use_id":"t1","content":"[exit 0]","exit_code":0}),
            // A result for a call that is not in the window (tail-bounded read).
            json!({"kind":"tool_result","tool_use_id":"ghost","content":"[exit 0]","exit_code":0}),
            json!({"kind":"user","content":"why did you stop?"}),
        ]);
        assert_eq!(messages.len(), 3, "{messages:#?}");
        let carrier = messages[2]["content"].as_array().unwrap();
        let results: Vec<&Value> = carrier.iter().filter(|b| b["type"] == "tool_result").collect();
        assert_eq!(results.len(), 2, "both tool_use ids are answered, the ghost is dropped");
        assert_eq!(results[0]["tool_use_id"], "t1");
        assert_eq!(results[1]["tool_use_id"], "t2");
        assert_eq!(results[1]["is_error"], true);
        let repaired = results[1]["content"].as_str().unwrap();
        assert!(repaired.contains("no result recorded"));
        assert!(repaired.contains("may already have happened"));
        // The operator's next prompt rides in the same user message as the
        // results it follows — one valid user turn, not two in a row.
        assert_eq!(carrier[2], json!({"type":"text","text":"why did you stop?"}));
    }

    #[test]
    fn thinking_blocks_are_dropped_and_large_results_are_bounded() {
        let big = "x".repeat(HISTORY_TOOL_RESULT_MAX_BYTES + 500) + "\n\n[exit 0]";
        let messages = replay(&[
            json!({"kind":"user","content":"q"}),
            json!({"kind":"assistant","content":[
                {"type":"thinking","thinking":"private","signature":"sig"},
                {"type":"tool_use","id":"t","name":"bash","input":{"command":"cat big"}}
            ]}),
            json!({"kind":"tool_result","tool_use_id":"t","content":big,"exit_code":0}),
        ]);
        let asst = messages[1]["content"].as_array().unwrap();
        assert!(asst.iter().all(|b| b["type"] != "thinking"));
        let body = messages[2]["content"][0]["content"].as_str().unwrap();
        assert!(body.len() < HISTORY_TOOL_RESULT_MAX_BYTES + 200);
        assert!(body.contains("bytes omitted from replayed history"));
        assert!(body.ends_with("[exit 0]"));
    }

    #[test]
    fn replayed_result_shape_is_stable_as_new_rounds_append() {
        let mut events = Vec::new();
        let body = format!("{}\n\n[exit 0]", "o".repeat(HISTORY_TOOL_RESULT_MAX_BYTES + 500));
        for i in 0..3 {
            events.push(json!({"kind":"user","content":format!("q{i}")}));
            events.push(json!({"kind":"assistant","content":[{"type":"tool_use","id":format!("t{i}"),"name":"bash","input":{"command":format!("cmd{i}")}}]}));
            events.push(json!({"kind":"tool_result","tool_use_id":format!("t{i}"),"content":body,"exit_code":0}));
            events.push(json!({"kind":"assistant","content":[{"type":"text","text":format!("a{i}")}]}));
        }
        let first = replay(&events);
        events.push(json!({"kind":"user","content":"q3"}));
        events.push(json!({"kind":"assistant","content":[{"type":"text","text":"a3"}]}));
        let extended = replay(&events);
        assert_eq!(first, extended[..first.len()]);
        assert!(first[2]["content"][0]["content"]
            .as_str()
            .unwrap()
            .contains("bytes omitted from replayed history"));
        assert_eq!(first[2]["content"][0]["tool_use_id"], "t0");
    }

    #[test]
    fn skill_expansion_is_the_exact_user_content_on_resume() {
        let expanded = "The operator invoked the skill `todo`.\nOperator request: list";
        let messages = replay(&[
            json!({
                "kind": "user",
                "content": "[skill:todo] list",
                "model_content": expanded,
            }),
            json!({"kind":"assistant","content":[{"type":"text","text":"done"}]}),
        ]);
        assert_eq!(messages[0]["content"], expanded);
    }

    #[test]
    fn one_oversized_history_message_is_dropped_and_env_overrides_the_cap() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LAZAR_SESSION_HISTORY_MAX_BYTES", "500000");
        assert_eq!(resolve_history_cap(TEST_HISTORY_CAP), 500_000);
        std::env::remove_var("LAZAR_SESSION_HISTORY_MAX_BYTES");
        assert_eq!(resolve_history_cap(TEST_HISTORY_CAP), TEST_HISTORY_CAP);
        let messages = vec![json!({
            "role": "user",
            "content": "x".repeat(TEST_HISTORY_CAP),
        })];
        let kept = truncate_to_cap(messages, TEST_HISTORY_CAP);
        assert!(serialized_size(&kept) <= TEST_HISTORY_CAP);
    }

    #[test]
    fn consecutive_operator_prompts_fold_and_history_opens_with_the_operator() {
        let messages = replay(&[
            json!({"kind":"assistant","content":[{"type":"text","text":"orphan from before the window"}]}),
            json!({"kind":"user","content":"test"}),
            json!({"kind":"user","content":"hello"}),
            json!({"kind":"assistant","content":[{"type":"text","text":"hi"}]}),
        ]);
        assert_eq!(messages[0], json!({"role":"user","content":"test\n\nhello"}));
        assert_eq!(messages[1]["content"][0]["text"], "hi");
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn truncation_never_starts_on_a_tool_result_carrier() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LAZAR_SESSION_HISTORY_MAX_BYTES", "260");
        let messages = truncate_to_byte_cap(vec![
            json!({"role":"user","content":"old prompt that will be dropped for size"}),
            json!({"role":"assistant","content":[{"type":"tool_use","id":"hist_1","name":"bash","input":{"command":"ls"}}]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"hist_1","content":"[exit 0]"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"done"}]}),
            json!({"role":"user","content":"recent prompt"}),
            json!({"role":"assistant","content":[{"type":"text","text":"recent answer"}]}),
        ]);
        std::env::remove_var("LAZAR_SESSION_HISTORY_MAX_BYTES");
        assert!(is_operator_message(&messages[0]), "{messages:#?}");
        assert_eq!(messages[0]["content"], "recent prompt");
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn truncate_linear_matches_old_semantics() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Same fixture as truncate_to_byte_cap_keeps_recent_messages — the
        // linear rewrite must produce byte-identical output to the old
        // per-drop-reserialize algorithm on the existing test cases.
        std::env::set_var("LAZAR_SESSION_HISTORY_MAX_BYTES", "200");
        let messages = vec![
            json!({"role":"user","content":"old user 1"}),
            json!({"role":"assistant","content":"old asst 1"}),
            json!({"role":"user","content":"recent user"}),
            json!({"role":"assistant","content":"recent asst"}),
        ];
        let expected = messages.clone();
        let result = truncate_to_byte_cap(messages);
        assert_eq!(result, expected);
        std::env::remove_var("LAZAR_SESSION_HISTORY_MAX_BYTES");
    }

    #[test]
    fn load_messages_tail_bound_skips_partial_line() {
        // Calls load_messages_from_path directly with a temp file, so no
        // LAZAR_HOME mutation is needed — only the cap env var, guarded
        // by ENV_LOCK like every other test that touches it.
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        const CAP: usize = 300;
        std::env::set_var("LAZAR_SESSION_HISTORY_MAX_BYTES", CAP.to_string());

        let dir = std::env::temp_dir().join(format!(
            "lazar-session-tail-bound-{}-{}",
            std::process::id(),
            crate::now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tailtest.jsonl");

        // Content marker planted in the OLD real user messages (not just
        // the tool_result filler) so the assertion actually proves the
        // tail-bound read excluded the cut-off region, rather than just
        // exercising the always-dropped tool_result branch.
        let filler_marker = "OLD_FILLER_MARKER_CUT_OFF";
        let mut lines: Vec<String> = Vec::new();
        for i in 0..50 {
            lines.push(
                json!({"kind": "user", "content": format!("{filler_marker} old msg {i}")})
                    .to_string(),
            );
            // Large tool_result lines inflate the file well past the read
            // budget (4x cap) without contributing any survivable message —
            // they have no matching tool_use, so the loader drops them as
            // orphans regardless of tail-bounding.
            lines.push(json!({"kind": "tool_result", "content": "z".repeat(2000)}).to_string());
        }
        lines.push(json!({"kind": "user", "content": "recent question"}).to_string());
        lines.push(json!({"kind": "assistant", "content": "recent answer"}).to_string());

        fs::write(&path, lines.join("\n") + "\n").unwrap();

        let file_len = fs::metadata(&path).unwrap().len();
        assert!(
            file_len as usize > CAP * 4,
            "fixture must exceed the tail-bound read budget to exercise the seek path"
        );

        let messages = load_messages_from_path(&path);

        assert!(
            !messages.is_empty(),
            "expected recent messages to survive the tail-bound read"
        );
        assert_eq!(
            messages[0].get("role").and_then(|v| v.as_str()),
            Some("user"),
            "loaded history must start with a user-role message"
        );
        for m in &messages {
            let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
            assert!(
                !content.contains(filler_marker),
                "loaded message unexpectedly contains cut-off-region content: {content:?}"
            );
        }

        std::env::remove_var("LAZAR_SESSION_HISTORY_MAX_BYTES");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_tool_history_keeps_the_latest_user_turn() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LAZAR_SESSION_HISTORY_MAX_BYTES", "4000");
        struct ClearCap;
        impl Drop for ClearCap {
            fn drop(&mut self) {
                std::env::remove_var("LAZAR_SESSION_HISTORY_MAX_BYTES");
            }
        }
        let _clear = ClearCap;
        let mut messages = vec![json!({"role":"user","content":"original task"})];
        for index in 0..12 {
            messages.push(json!({"role":"assistant","content":[{"type":"text","text":"x".repeat(800)},{"type":"tool_use","id":format!("t{index}"),"name":"bash","input":{"command":"printf x"}}]}));
            messages.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":format!("t{index}"),"content":"y".repeat(800)}]}));
        }
        messages.push(json!({"role":"user","content":"latest question"}));
        let kept = truncate_to_byte_cap(messages);
        assert!(is_operator_message(kept.first().unwrap()), "{kept:?}");
        let encoded = serde_json::to_string(&kept).unwrap();
        assert!(encoded.contains("latest question"), "{encoded}");
        assert!(encoded.len() <= 4000, "len {}", encoded.len());
    }

    #[test]
    fn invalid_utf8_before_an_oversized_turn_still_continues() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LAZAR_SESSION_HISTORY_MAX_BYTES", "64");
        struct ClearCap;
        impl Drop for ClearCap {
            fn drop(&mut self) {
                std::env::remove_var("LAZAR_SESSION_HISTORY_MAX_BYTES");
            }
        }
        let _clear = ClearCap;
        let dir = std::env::temp_dir().join(format!(
            "lazar-session-bad-utf8-{}-{}",
            std::process::id(),
            crate::now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.jsonl");
        let mut bytes = vec![0xff, b'\n'];
        bytes.extend(
            json!({"kind":"user","content":"x".repeat(200)})
                .to_string()
                .into_bytes(),
        );
        bytes.push(b'\n');
        fs::write(&path, bytes).unwrap();

        // An oversized turn is shortened, never refused: compaction owns the
        // window now, and bad UTF-8 must not hide the conversation.
        let messages = load_messages_checked_from_path(&path).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(is_operator_message(&messages[0]));
        let _ = fs::remove_dir_all(&dir);
    }
}
