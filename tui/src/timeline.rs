//! Pure mapping from a `lazar jobs --show <id>` timeline (raw kernel JSON)
//! to the transcript's own `Block` vocabulary, so the job-detail screen
//! renders with exactly the same look (`render::transcript::render_block`)
//! as the live chat transcript instead of a bespoke layout.

use crate::transcript::{BannerKind, Block, VerifyClaim, VerifyStatus};
use serde_json::Value;

fn text_field(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Steps: `{"kind":"prompt","text"}` | `{"kind":"text","text"}` |
/// `{"kind":"tool","id","command","exit_code","output"}` |
/// `{"kind":"verify","status","claims":[{claim,ok,detail}]}` |
/// `{"kind":"end","stop_reason","duration_ms","input_tokens","output_tokens"}`
/// | `{"kind":"error","text"}`. Unknown kinds, and a `timeline` that isn't a
/// JSON array (e.g. the kernel's error shape), map to no blocks.
pub fn timeline_blocks(timeline: &Value) -> Vec<Block> {
    let mut blocks = Vec::new();
    let Some(steps) = timeline.as_array() else {
        return blocks;
    };
    for step in steps {
        match step.get("kind").and_then(Value::as_str).unwrap_or("") {
            "prompt" => {
                // Distinct from a live chat turn's boxed `Block::User`: just
                // a dim label, so it reads as "here's the task", not as
                // something the operator typed into this session.
                blocks.push(Block::Marker { text: "task".into() });
                blocks.push(Block::User { text: text_field(step, "text") });
            }
            "text" => blocks.push(Block::Assistant {
                markdown: text_field(step, "text"),
                streaming: false,
            }),
            "operator" => {
                // A follow-up message from the operator: a steer (queued
                // into a running turn) or a reply that started a new turn
                // on a finished job. Same bubble as a live chat turn; a
                // steer additionally gets a small dim tag underneath.
                blocks.push(Block::User { text: text_field(step, "text") });
                if step.get("steer").and_then(Value::as_bool).unwrap_or(false) {
                    blocks.push(Block::Marker { text: "steered mid-task".into() });
                }
            }
            "tool" => blocks.push(Block::Tool {
                id: text_field(step, "id"),
                command: text_field(step, "command"),
                output: step.get("output").and_then(Value::as_str).map(str::to_string),
                exit_code: step.get("exit_code").and_then(Value::as_i64).map(|n| n as i32),
                // Folded one-line header by default; the detail screen's
                // Enter/Ctrl+T unfolds every tool at once.
                folded: true,
                running: false,
                count: 1,
            }),
            "verify" => {
                let status = match step.get("status").and_then(Value::as_str) {
                    Some("passed") => VerifyStatus::Passed,
                    _ => VerifyStatus::Failed,
                };
                let claims: Vec<VerifyClaim> = step
                    .get("claims")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .map(|c| VerifyClaim {
                                claim: text_field(c, "claim"),
                                ok: c.get("ok").and_then(Value::as_bool).unwrap_or(false),
                                detail: text_field(c, "detail"),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let tools_run = claims.len() as u64;
                blocks.push(Block::Verify {
                    status,
                    // The step doesn't carry attempt/final — this timeline
                    // shows one already-resolved step, not a live retry loop.
                    attempt: 1,
                    final_: true,
                    tools_run,
                    claims,
                });
            }
            "end" => {
                let stop_reason = step
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("end_turn");
                let secs = step.get("duration_ms").and_then(Value::as_u64).unwrap_or(0) / 1000;
                let input_k = step.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as f64 / 1000.0;
                let output_k = step.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as f64 / 1000.0;
                blocks.push(Block::Marker {
                    text: format!("ended: {stop_reason} · {secs}s · ↓{input_k:.1}k ↑{output_k:.1}k tokens"),
                });
            }
            "error" => blocks.push(Block::Banner {
                kind: BannerKind::Error,
                text: text_field(step, "text"),
            }),
            _ => {}
        }
    }
    blocks
}

/// True when `blocks` already contains a user bubble with exactly this
/// text — used to drop the job-detail composer's optimistic "sending…"
/// bubble once a fresh `jobs --show` timeline catches up to it.
pub fn timeline_contains_user_text(blocks: &[Block], text: &str) -> bool {
    blocks.iter().any(|block| matches!(block, Block::User { text: t } if t == text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_every_step_kind() {
        let timeline = json!([
            {"kind": "prompt", "text": "do the thing"},
            {"kind": "text", "text": "on it"},
            {"kind": "tool", "id": "t1", "command": "ls", "exit_code": 0, "output": "a\nb"},
            {"kind": "verify", "status": "passed", "claims": [{"claim": "exists=/x", "ok": true, "detail": "found"}]},
            {"kind": "end", "stop_reason": "end_turn", "duration_ms": 12_000, "input_tokens": 1500, "output_tokens": 300},
            {"kind": "error", "text": "boom"},
        ]);
        let blocks = timeline_blocks(&timeline);
        assert_eq!(blocks.len(), 7, "{blocks:?}");
        assert!(matches!(&blocks[0], Block::Marker { text } if text == "task"));
        assert!(matches!(&blocks[1], Block::User { text } if text == "do the thing"));
        assert!(matches!(&blocks[2], Block::Assistant { markdown, streaming: false } if markdown == "on it"));
        assert!(matches!(
            &blocks[3],
            Block::Tool { command, folded: true, exit_code: Some(0), output: Some(o), .. }
                if command == "ls" && o == "a\nb"
        ));
        assert!(matches!(
            &blocks[4],
            Block::Verify { status: VerifyStatus::Passed, tools_run: 1, claims, .. } if claims.len() == 1
        ));
        assert!(matches!(
            &blocks[5],
            Block::Marker { text }
                if text.starts_with("ended: end_turn")
                    && text.contains("12s")
                    && text.contains("↓1.5k")
                    && text.contains("↑0.3k")
        ));
        assert!(matches!(&blocks[6], Block::Banner { kind: BannerKind::Error, text } if text == "boom"));
    }

    #[test]
    fn a_failed_or_missing_verify_status_maps_to_failed() {
        let timeline = json!([{"kind": "verify", "status": "failed", "claims": []}]);
        let blocks = timeline_blocks(&timeline);
        assert!(matches!(&blocks[0], Block::Verify { status: VerifyStatus::Failed, tools_run: 0, .. }));

        let timeline = json!([{"kind": "verify", "claims": []}]);
        let blocks = timeline_blocks(&timeline);
        assert!(matches!(&blocks[0], Block::Verify { status: VerifyStatus::Failed, .. }));
    }

    #[test]
    fn unknown_step_kinds_and_a_non_array_timeline_are_ignored() {
        assert!(timeline_blocks(&json!(null)).is_empty());
        assert!(timeline_blocks(&json!({"error": "unknown job"})).is_empty());
        assert!(timeline_blocks(&json!([{"kind": "future_kind", "x": 1}])).is_empty());
    }

    #[test]
    fn operator_steps_map_to_user_blocks_and_a_steer_gets_a_dim_tag() {
        let blocks = timeline_blocks(&json!([
            {"kind": "operator", "text": "also check the tests"},
            {"kind": "operator", "text": "stop and rerun", "steer": true},
        ]));
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert!(matches!(&blocks[0], Block::User { text } if text == "also check the tests"));
        assert!(matches!(&blocks[1], Block::User { text } if text == "stop and rerun"));
        assert!(matches!(&blocks[2], Block::Marker { text } if text == "steered mid-task"));
    }

    #[test]
    fn timeline_contains_user_text_matches_exact_bubbles_only() {
        let blocks = timeline_blocks(&json!([{"kind": "operator", "text": "go faster"}]));
        assert!(timeline_contains_user_text(&blocks, "go faster"));
        assert!(!timeline_contains_user_text(&blocks, "go faster please"));
        assert!(!timeline_contains_user_text(&[], "go faster"));
    }

    #[test]
    fn a_step_missing_optional_fields_still_maps() {
        let blocks = timeline_blocks(&json!([{"kind": "tool", "id": "t1", "command": "pwd"}]));
        assert!(matches!(
            &blocks[0],
            Block::Tool { id, command, output: None, exit_code: None, folded: true, .. }
                if id == "t1" && command == "pwd"
        ));
    }
}
