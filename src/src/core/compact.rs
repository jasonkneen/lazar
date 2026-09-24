//! In-turn context compaction.
//!
//! The loop never refuses a turn for size. When a request would outgrow the
//! model's window (`context::Window`), the kernel shrinks the conversation in
//! stages, cheapest first, and always leaves a valid Messages-API shape:
//!
//! 1. `elide_old_tool_results` — old tool outputs and inputs keep a head and
//!    tail; the full text stays in the session log.
//! 2. summary — the oldest rounds are replaced by one operator message holding
//!    a summary (model-written when possible, `digest` otherwise) plus the
//!    operator's latest request verbatim. The cut always lands on an
//!    assistant message so every tool_result still follows its tool_use.
//! 3. `truncate_items` — last resort for a single huge item.
//!
//! Everything here is pure; the model call for stage 2 lives in main.rs.

use serde_json::{json, Value};

use super::context;
use crate::text::head_bytes;

/// Tool-result carriers at the end of the conversation left untouched by
/// elision: the model is usually mid-way through using them.
pub(crate) const KEEP_RECENT_TOOL_ROUNDS: usize = 2;
/// Bytes an elided tool output or input keeps (head + tail).
pub(crate) const ELIDED_ITEM_BYTES: usize = 1_200;
/// Per-item bytes when rendering history for the summariser.
const SUMMARY_ITEM_BYTES: usize = 1_500;

/// Operator-role nudge the kernel sends when a reply stopped at the output
/// budget. Replayed from the log as `continuation` events.
pub(crate) const CONTINUE_PROMPT: &str = "[kernel: your previous reply stopped at the output limit. Continue exactly where it stopped; do not repeat what you already wrote.]";

pub(crate) const SUMMARY_SYSTEM_PROMPT: &str = "You compress an AI agent's working history so it can keep going with a smaller context. Write a dense, factual summary for the agent itself: the operator's goals and constraints, decisions made, what was tried and what happened (commands, files, key outputs, errors), current state, and what remains to do. Keep exact paths, identifiers, numbers and error text. No preamble, no advice.";

/// Serialized size of a message list as it will appear in a request.
/// Attached images count as their token cost, not their base64 size.
pub(crate) fn bytes(messages: &[Value]) -> usize {
    crate::core::images::weighted_len(&Value::Array(messages.to_vec()))
}

/// Keep the head and tail of `text` within `max` bytes, marking the gap.
pub(crate) fn elide(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let head = head_bytes(text, max * 2 / 3);
    let tail_len = max.saturating_sub(head.len());
    let mut start = text.len().saturating_sub(tail_len);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let omitted = start.saturating_sub(head.len());
    format!(
        "{head}\n[… {omitted} bytes compacted out of context; the full text is in the session log …]\n{}",
        &text[start..]
    )
}

fn is_tool_result_carrier(message: &Value) -> bool {
    message["role"] == "user"
        && message["content"]
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
}

/// Stage 1. Returns true when anything changed.
pub(crate) fn elide_old_tool_results(messages: &mut [Value], keep_recent: usize, max: usize) -> bool {
    let carriers: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| is_tool_result_carrier(m))
        .map(|(i, _)| i)
        .collect();
    // Everything before the first protected carrier is old.
    if carriers.len() <= keep_recent {
        return false;
    }
    let boundary = carriers[carriers.len() - keep_recent];
    let mut changed = false;
    for message in &mut messages[..boundary] {
        let Some(blocks) = message["content"].as_array_mut() else {
            continue;
        };
        for block in blocks {
            match block["type"].as_str() {
                Some("tool_result") => {
                    if let Some(text) = block["content"].as_str() {
                        if text.len() > max {
                            block["content"] = json!(elide(text, max));
                            changed = true;
                        }
                    }
                }
                Some("tool_use") => {
                    if let Some(command) = block["input"]["command"].as_str() {
                        if command.len() > max {
                            block["input"]["command"] = json!(elide(command, max));
                            changed = true;
                        }
                    }
                }
                Some("text") => {
                    if let Some(text) = block["text"].as_str() {
                        if text.len() > max * 4 {
                            block["text"] = json!(elide(text, max * 4));
                            changed = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    changed
}

/// Stage 2 cut point: the earliest assistant message (index >= 1) whose tail
/// fits in `keep_tail` bytes, else the last assistant message. `None` when
/// there is nothing before an assistant message to summarise.
pub(crate) fn summary_cut(messages: &[Value], keep_tail: usize) -> Option<usize> {
    let assistants: Vec<usize> = messages
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, m)| m["role"] == "assistant")
        .map(|(i, _)| i)
        .collect();
    let last = *assistants.last()?;
    Some(
        assistants
            .into_iter()
            .find(|&i| bytes(&messages[i..]) <= keep_tail)
            .unwrap_or(last),
    )
}

/// The latest operator-typed message in `messages`, if any.
pub(crate) fn last_operator_text(messages: &[Value]) -> Option<String> {
    messages.iter().rev().find_map(|m| {
        if m["role"] != "user" {
            return None;
        }
        match &m["content"] {
            Value::String(text) => Some(text.clone()),
            Value::Array(blocks) => blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .last()
                .map(str::to_string),
            _ => None,
        }
    })
}

/// Plain-text rendering of `head` for the summariser, bounded to `max` bytes
/// by keeping the most recent part.
pub(crate) fn transcript(head: &[Value], max: usize) -> String {
    let mut out = String::new();
    for message in head {
        let role = message["role"].as_str().unwrap_or("?");
        match &message["content"] {
            Value::String(text) => {
                out.push_str(&format!("\n[{role}]\n{}\n", elide(text, SUMMARY_ITEM_BYTES * 2)));
            }
            Value::Array(blocks) => {
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") => out.push_str(&format!(
                            "\n[{role}]\n{}\n",
                            elide(block["text"].as_str().unwrap_or(""), SUMMARY_ITEM_BYTES * 2)
                        )),
                        Some("tool_use") => out.push_str(&format!(
                            "\n[tool call]\n$ {}\n",
                            elide(block["input"]["command"].as_str().unwrap_or(""), SUMMARY_ITEM_BYTES)
                        )),
                        Some("tool_result") => out.push_str(&format!(
                            "\n[tool result]\n{}\n",
                            elide(block["content"].as_str().unwrap_or(""), SUMMARY_ITEM_BYTES)
                        )),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if out.len() <= max {
        return out;
    }
    let mut start = out.len() - max;
    while !out.is_char_boundary(start) {
        start += 1;
    }
    format!("[… earlier history omitted …]\n{}", &out[start..])
}

/// Fallback summary when the model call fails: the operator's prompts and
/// the agent's words, newest kept when over `max`.
pub(crate) fn digest(head: &[Value], max: usize) -> String {
    let mut out = String::from("Mechanical digest (the summariser was unavailable):\n");
    for message in head {
        match (&message["role"], &message["content"]) {
            (role, Value::String(text)) if role == "user" => {
                out.push_str(&format!("- operator: {}\n", elide(text, 600)));
            }
            (_, Value::Array(blocks)) => {
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") if message["role"] == "assistant" => out.push_str(&format!(
                            "- agent: {}\n",
                            elide(block["text"].as_str().unwrap_or(""), 400)
                        )),
                        Some("tool_use") => out.push_str(&format!(
                            "- ran: {}\n",
                            elide(block["input"]["command"].as_str().unwrap_or(""), 200)
                        )),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    elide(&out, max)
}

/// Replace `messages[..cut]` with one operator message carrying `summary` and
/// the latest operator request from the replaced part.
pub(crate) fn replace_head(messages: &mut Vec<Value>, cut: usize, summary: &str) {
    let request = last_operator_text(&messages[..cut]).unwrap_or_default();
    let mut text = format!(
        "[Context compacted by the kernel. Summary of the earlier conversation:]\n{}",
        summary.trim()
    );
    if !request.is_empty() {
        text.push_str(&format!(
            "\n\n[The operator's most recent request, verbatim:]\n{}",
            elide(&request, context::MAX_CONTEXT_ITEM_BYTES / 4)
        ));
    }
    messages.splice(..cut, [json!({"role": "user", "content": text})]);
}

/// Stage 3: bound every text-bearing item to `max` bytes.
pub(crate) fn truncate_items(messages: &mut [Value], max: usize) -> bool {
    let mut changed = false;
    for message in messages {
        if let Some(text) = message["content"].as_str() {
            if text.len() > max {
                message["content"] = json!(elide(text, max));
                changed = true;
            }
            continue;
        }
        let Some(blocks) = message["content"].as_array_mut() else {
            continue;
        };
        for block in blocks {
            for field in ["text", "content"] {
                if let Some(text) = block[field].as_str() {
                    if text.len() > max {
                        block[field] = json!(elide(text, max));
                        changed = true;
                    }
                }
            }
            if let Some(command) = block["input"]["command"].as_str() {
                if command.len() > max {
                    block["input"]["command"] = json!(elide(command, max));
                    changed = true;
                }
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(i: usize, out: usize) -> [Value; 2] {
        [
            json!({"role":"assistant","content":[{"type":"text","text":"working"},{"type":"tool_use","id":format!("t{i}"),"name":"bash","input":{"command":"cat big"}}]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":format!("t{i}"),"content":"o".repeat(out)}]}),
        ]
    }

    fn conversation(rounds: usize, out: usize) -> Vec<Value> {
        let mut messages = vec![json!({"role":"user","content":"fix the bug"})];
        for i in 0..rounds {
            messages.extend(round(i, out));
        }
        messages
    }

    fn valid(messages: &[Value]) -> bool {
        messages.first().is_some_and(|m| m["role"] == "user")
            && messages.windows(2).all(|w| w[0]["role"] != w[1]["role"])
            && messages.iter().enumerate().all(|(i, m)| {
                m["content"].as_array().map_or(true, |blocks| {
                    blocks.iter().filter(|b| b["type"] == "tool_result").all(|b| {
                        i > 0 && messages[i - 1]["content"].as_array().is_some_and(|prev| {
                            prev.iter().any(|p| p["id"] == b["tool_use_id"])
                        })
                    })
                })
            })
    }

    #[test]
    fn elide_keeps_head_and_tail_and_is_utf8_safe() {
        let text = format!("HEAD{}TAIL", "é".repeat(5_000));
        let short = elide(&text, 600);
        assert!(short.starts_with("HEAD") && short.ends_with("TAIL"), "{short}");
        assert!(short.len() < 800);
        assert_eq!(elide("small", 600), "small");
    }

    #[test]
    fn elision_spares_the_most_recent_rounds() {
        let mut messages = conversation(5, 10_000);
        assert!(elide_old_tool_results(&mut messages, 2, ELIDED_ITEM_BYTES));
        let sizes: Vec<usize> = messages
            .iter()
            .filter(|m| is_tool_result_carrier(m))
            .map(|m| m["content"][0]["content"].as_str().unwrap().len())
            .collect();
        assert!(sizes[..3].iter().all(|&n| n < 2_000), "{sizes:?}");
        assert_eq!(&sizes[3..], &[10_000, 10_000]);
        assert!(valid(&messages));
        let mut few = conversation(2, 10_000);
        assert!(!elide_old_tool_results(&mut few, 2, ELIDED_ITEM_BYTES));
    }

    #[test]
    fn summary_replaces_the_head_and_keeps_the_shape_valid() {
        let mut messages = conversation(8, 3_000);
        let before = bytes(&messages);
        let cut = summary_cut(&messages, before / 3).unwrap();
        assert_eq!(messages[cut]["role"], "assistant");
        let head_text = transcript(&messages[..cut], 10_000);
        assert!(head_text.contains("fix the bug") || head_text.contains("earlier history omitted"));
        replace_head(&mut messages, cut, "did things");
        assert!(valid(&messages), "{messages:#?}");
        let first = messages[0]["content"].as_str().unwrap();
        assert!(first.contains("did things") && first.contains("fix the bug"), "{first}");
        assert!(bytes(&messages) < before / 2);
    }

    #[test]
    fn summary_cut_needs_something_to_summarise() {
        assert_eq!(summary_cut(&[json!({"role":"user","content":"hi"})], 10), None);
        let messages = conversation(1, 100_000);
        // Tail never fits: fall back to the last assistant message.
        assert_eq!(summary_cut(&messages, 10), Some(1));
    }

    #[test]
    fn digest_and_truncation_bound_output() {
        let messages = conversation(40, 500);
        let digest = digest(&messages, 2_000);
        assert!(digest.len() < 2_200 && digest.contains("ran: cat big"));
        let mut huge = vec![json!({"role":"user","content":"x".repeat(50_000)})];
        assert!(truncate_items(&mut huge, 1_000));
        assert!(bytes(&huge) < 1_500);
    }
}
