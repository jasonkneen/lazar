//! lazar session continuity — persistent multi-turn conversations.
//!
//! When `lazar -p ... --session <id>` is invoked:
//!   1. If `logs/sessions/<id>.jsonl` exists, prior turns are loaded and
//!      prepended to the message array. The agent sees actual conversation
//!      history, not just ambient state.
//!   2. New events (user prompt, assistant turns, tool results) are
//!      appended to that session log AS WELL AS to stream.jsonl.
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
    io::{BufRead, BufReader, Write},
    path::PathBuf,
};

use crate::{lazar_home, now_millis};

/// Default cap for loaded session history. Once decoded JSON messages
/// exceed this byte count, oldest pairs are dropped from the front
/// until under the cap. Override via LAZAR_SESSION_HISTORY_MAX_BYTES.
const DEFAULT_SESSION_HISTORY_MAX_BYTES: usize = 200_000;

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

/// Path to the session log for this id. Caller is responsible for
/// validating the id first.
pub fn session_log_path(id: &str) -> PathBuf {
    lazar_home()
        .join("logs")
        .join("sessions")
        .join(format!("{id}.jsonl"))
}

/// Find the most recently modified session log under
/// `$LAZAR_HOME/logs/sessions/`. Returns the session id (filename
/// stem), or None if no session logs exist or the directory is
/// missing. Used by `--resume`.
pub fn find_newest_session() -> Option<String> {
    let dir = lazar_home().join("logs").join("sessions");
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

/// Read prior turns from a session log and convert them into the
/// Anthropic Messages API format. Returns an empty Vec if the log
/// doesn't exist (first turn).
///
/// The returned messages are guaranteed to:
///   - Start with a user-role message
///   - Have alternating roles (user → assistant → user → ...)
///   - Total under LAZAR_SESSION_HISTORY_MAX_BYTES
pub fn load_messages(id: &str) -> Vec<Value> {
    let path = session_log_path(id);
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return vec![], // no prior turns
    };

    let reader = BufReader::new(file);
    let mut messages: Vec<Value> = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    let flush_tool_results =
        |messages: &mut Vec<Value>, pending: &mut Vec<Value>| {
            if !pending.is_empty() {
                messages.push(json!({
                    "role": "user",
                    "content": std::mem::take(pending),
                }));
            }
        };

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
        let kind = event.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "user" => {
                flush_tool_results(&mut messages, &mut pending_tool_results);
                if let Some(content) = event.get("content") {
                    messages.push(json!({
                        "role": "user",
                        "content": content,
                    }));
                }
            }
            "assistant" => {
                flush_tool_results(&mut messages, &mut pending_tool_results);
                if let Some(content) = event.get("content") {
                    messages.push(json!({
                        "role": "assistant",
                        "content": content,
                    }));
                }
            }
            "tool_result" => {
                if let (Some(id), Some(content)) = (
                    event.get("tool_use_id").cloned(),
                    event.get("content").cloned(),
                ) {
                    pending_tool_results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": content,
                    }));
                }
            }
            _ => {}
        }
    }
    flush_tool_results(&mut messages, &mut pending_tool_results);

    truncate_to_byte_cap(messages)
}

/// Drop oldest message pairs until the remaining set fits under the
/// byte cap. Always keeps the conversation valid: starts with a user
/// message, alternating roles preserved.
fn truncate_to_byte_cap(mut messages: Vec<Value>) -> Vec<Value> {
    let cap = std::env::var("LAZAR_SESSION_HISTORY_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n: &usize| *n > 0)
        .unwrap_or(DEFAULT_SESSION_HISTORY_MAX_BYTES);

    let mut total = serialized_size(&messages);
    while total > cap && messages.len() > 1 {
        // Drop the oldest message. Repeat until under cap or only one left.
        messages.remove(0);
        total = serialized_size(&messages);
    }

    // Ensure the first remaining message is user-role. If we removed an
    // odd number from the front and the new head is assistant-role, drop
    // it too so the API doesn't reject the request.
    while messages
        .first()
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
        != Some("user")
        && !messages.is_empty()
    {
        messages.remove(0);
    }

    messages
}

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
pub fn append_session(id: &str, mut event: Value) {
    let path = session_log_path(id);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Some(obj) = event.as_object_mut() {
        obj.entry("ts_ms".to_string())
            .or_insert_with(|| json!(now_millis()));
    }

    let line = match serde_json::to_string(&event) {
        Ok(s) => s,
        Err(_) => return,
    };

    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }

    match opts.open(&path) {
        Ok(mut f) => {
            let _ = writeln!(f, "{line}");
        }
        Err(e) => {
            eprintln!(
                "[lazar] WARN: failed to append session log {}: {e}",
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        std::env::set_var("LAZAR_SESSION_HISTORY_MAX_BYTES", "200");
        let messages = vec![
            json!({"role":"user","content":"old user 1"}),
            json!({"role":"assistant","content":"old asst 1"}),
            json!({"role":"user","content":"recent user"}),
            json!({"role":"assistant","content":"recent asst"}),
        ];
        let result = truncate_to_byte_cap(messages);
        // Should drop oldest until under cap
        assert!(serialized_size(&result) <= 200 || result.len() == 1);
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
}
