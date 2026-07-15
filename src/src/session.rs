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

/// Path to the trusted, kernel-owned session log for this id. The sandbox
/// profile must not grant tools write access to this tree. Caller is responsible for
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
/// IMPORTANT: tool_use blocks and tool_result events are STRIPPED from the
/// loaded history. Tool ids are provider-specific (Anthropic, MiniMax,
/// OpenRouter, etc all use different formats), and lazar's cascading
/// provider failover means a session log can mix providers. Sending a
/// `tool_result` with a foreign tool_use_id back to any single provider
/// produces an "unknown tool id" 400.
///
/// Truncation can also break tool_use→tool_result pairing (drop the
/// assistant message and you have an orphan tool_result).
///
/// The clean, provider-portable, truncation-safe choice is: load only
/// text content. The agent loses fine-grained "what tools did I run"
/// detail in continued sessions, but stream.jsonl still has it for
/// archeology — and the multi-turn referential bug is fixed regardless.
///
/// The returned messages are guaranteed to:
///   - Start with a user-role message
///   - Contain only text content (no tool_use, no tool_result)
///   - Total under LAZAR_SESSION_HISTORY_MAX_BYTES
pub fn load_messages(id: &str) -> Vec<Value> {
    load_messages_from_path(&session_log_path(id))
}

/// Path-parameterized loader backing `load_messages`. Split out so tests
/// can exercise the tail-bounded read against a temp file directly,
/// without mutating the process-global LAZAR_HOME env var (which races
/// with other test modules under the multithreaded test runner).
fn load_messages_from_path(path: &Path) -> Vec<Value> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return vec![], // no prior turns
    };

    // Session logs are never rotated and mirror every stream event
    // (including tool_result output up to LAZAR_TOOL_OUTPUT_MAX_BYTES),
    // so a long-lived session can grow to megabytes even though only a
    // small, recent slice of it ever survives into loaded history. Avoid
    // parsing the whole file: read a bounded tail with headroom (4x the
    // resolved cap — tool_result/other discarded lines still count
    // against the read budget, so headroom keeps enough real user/
    // assistant text in view to fill the cap).
    let cap = resolve_history_cap();
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

    let mut messages: Vec<Value> = Vec::new();

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
                if let Some(content) = event.get("content") {
                    push_message(&mut messages, "user", content.clone());
                }
            }
            "assistant" => {
                // Strip tool_use blocks; keep only text blocks. If the
                // turn was tool-use-only, skip it entirely.
                if let Some(text) = extract_text_from_assistant_content(event.get("content")) {
                    if !text.is_empty() {
                        push_message(&mut messages, "assistant", json!(text));
                    }
                }
            }
            // tool_result events are intentionally dropped — see doc above.
            _ => {}
        }
    }

    truncate_to_byte_cap(messages)
}

/// Keep provider history valid after persisted tool blocks/results have been
/// stripped. A tool round can otherwise leave adjacent assistant messages.
fn push_message(messages: &mut Vec<Value>, role: &str, content: Value) {
    if let Some(last) = messages.last_mut() {
        let same_role = last.get("role").and_then(|v| v.as_str()) == Some(role);
        if same_role {
            if let (Some(existing), Some(next)) = (
                last.get("content").and_then(|v| v.as_str()),
                content.as_str(),
            ) {
                last["content"] = json!(format!("{existing}\n\n{next}"));
                return;
            }
        }
    }
    messages.push(json!({"role": role, "content": content}));
}

/// Pull the concatenated text out of an assistant content array,
/// dropping tool_use and any other non-text blocks. Returns None if
/// content is missing; returns Some("") if the turn was tool-only.
fn extract_text_from_assistant_content(content: Option<&Value>) -> Option<String> {
    let content = content?;
    // Assistant content can be either a plain string (rare, defensive)
    // or an array of content blocks (the normal Anthropic shape).
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let arr = content.as_array()?;
    let mut out = String::new();
    for block in arr {
        if block.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
        // tool_use, tool_result, image, etc — silently dropped.
    }
    Some(out)
}

/// Resolve the session-history byte cap, honoring
/// LAZAR_SESSION_HISTORY_MAX_BYTES when set to a positive value.
fn resolve_history_cap() -> usize {
    std::env::var("LAZAR_SESSION_HISTORY_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n: &usize| *n > 0)
        .unwrap_or(DEFAULT_SESSION_HISTORY_MAX_BYTES)
}

/// Drop oldest message pairs until the remaining set fits under the
/// byte cap. Always keeps the conversation valid: starts with a user
/// message, alternating roles preserved.
///
/// Each message's serialized size is computed exactly once and summed;
/// dropping from the front updates a running total instead of
/// re-serializing the whole array per removal, so this is linear in the
/// number of messages rather than O(dropped x total).
fn truncate_to_byte_cap(mut messages: Vec<Value>) -> Vec<Value> {
    let cap = resolve_history_cap();

    let sizes: Vec<usize> = messages.iter().map(message_size).collect();
    let mut total: usize = sizes.iter().sum();

    let mut drop_count = 0;
    while total > cap && messages.len() - drop_count > 1 {
        total -= sizes[drop_count];
        drop_count += 1;
    }
    if drop_count > 0 {
        messages.drain(0..drop_count);
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

/// Serialized size of a single message, used by `truncate_to_byte_cap`
/// to build the running-total size table once up front.
fn message_size(message: &Value) -> usize {
    serde_json::to_string(message).map(|s| s.len()).unwrap_or(0)
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
    use std::sync::Mutex;

    /// Serializes tests that read or mutate the process-global
    /// LAZAR_SESSION_HISTORY_MAX_BYTES env var. Rust runs tests on
    /// multiple threads by default, so unguarded set_var/remove_var races
    /// between tests in this module. Lock poison-tolerantly: a panicking
    /// test must not cascade-fail the others.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    #[test]
    fn push_message_merges_adjacent_roles_after_tool_stripping() {
        let mut messages = vec![json!({"role":"user","content":"inspect"})];
        push_message(&mut messages, "assistant", json!("I will inspect."));
        push_message(&mut messages, "assistant", json!("Inspection complete."));

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["content"], "I will inspect.\n\nInspection complete.");
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
            // tool_result is always dropped by the loader regardless of
            // tail-bounding.
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
}
