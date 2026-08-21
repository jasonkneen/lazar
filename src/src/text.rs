//! Small text utilities shared by the agent loop and the session loader.

use serde_json::Value;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_text_joins_text_blocks_and_skips_tool_use() {
        let content = json!([
            {"type": "text", "text": "one"},
            {"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "ls"}},
            {"type": "text", "text": "two"},
        ]);
        assert_eq!(extract_text(&content), "one\ntwo");
        assert_eq!(extract_text(&json!("plain")), "plain");
        assert_eq!(extract_text(&json!(null)), "");
    }

    #[test]
    fn head_bytes_backs_off_to_char_boundary() {
        let s = "─".repeat(30_000); // 3 bytes each
        let cut = head_bytes(&s, 65_536);
        assert_eq!(cut.len(), (65_536 / 3) * 3);
        assert!(s.is_char_boundary(cut.len()));
        assert_eq!(head_bytes("ab", 10), "ab");
    }
}
