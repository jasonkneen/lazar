//! Sub-agent jobs, as `lazar-spawn` records them, summarised for clients.
//!
//! `lazar-spawn` writes one record per job under
//! `user/workspace/subagent/jobs/<job-id>.json` (status `running` while the
//! child works, then `ok` / `fail` / `error`) and streams the child's
//! stream-json to the record's `output` log. `lazar jobs --json` turns those
//! into rows a status line can show: task, state, elapsed time, tool calls and
//! tokens so far. A `running` record whose process is gone reads as `lost`.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Finished jobs stay listed this long, so a status line can show the result.
pub(crate) const RECENT_MS: u128 = 10 * 60 * 1000;

fn alive(pid: i64) -> bool {
    pid > 0 && unsafe { libc::kill(pid as i32, 0) } == 0
}

fn mtime_ms(path: &Path) -> u128 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Tool calls and tokens from a child's stream-json log.
fn progress(output: &Path) -> (u64, u64, u64) {
    let Ok(text) = fs::read_to_string(output) else {
        return (0, 0, 0);
    };
    let (mut tools, mut input, mut output_tokens) = (0, 0, 0);
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
        match event["type"].as_str() {
            Some("tool_use") => tools += 1,
            // Per-request usage while the child runs.
            Some("context") => {
                input = event["input_tokens"].as_u64().unwrap_or(input);
                output_tokens += event["output_tokens"].as_u64().unwrap_or(0);
            }
            _ => {}
        }
    }
    (tools, input, output_tokens)
}

/// Rows for running jobs and jobs finished within `recent_ms`, newest first.
pub(crate) fn list(jobs_dir: &Path, now_ms: u128, recent_ms: u128) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(jobs_dir) else {
        return Vec::new();
    };
    let mut rows: Vec<(u128, Value)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".json") || name.ends_with(".spec.json") {
            continue;
        }
        let Ok(record) = fs::read_to_string(&path).map(|t| serde_json::from_str::<Value>(&t)) else {
            continue;
        };
        let Ok(record) = record else { continue };
        let modified = mtime_ms(&path);
        let mut status = record["status"].as_str().unwrap_or("unknown").to_string();
        let running = matches!(status.as_str(), "running" | "starting");
        if running && record["pid"].as_i64().is_some_and(|pid| !alive(pid)) {
            status = "lost".into();
        }
        let live = status == "running" || status == "starting";
        if !live && now_ms.saturating_sub(modified) > recent_ms {
            continue;
        }
        let started = record["started_ms"].as_u64().map(u128::from).unwrap_or(modified);
        let (tools, input_tokens, output_tokens) = record["output"]
            .as_str()
            .map(|o| progress(Path::new(o)))
            .unwrap_or((0, 0, 0));
        let summary = record["output"]
            .as_str()
            .map(|o| summarise(&timeline(Path::new(o))))
            .unwrap_or_default();
        let mut row = json!({
            "summary": summary,
            "age_s": now_ms.saturating_sub(modified) / 1000,
            "job_id": record["job_id"],
            "task": record["task"],
            "status": status,
            "elapsed_s": (if live { now_ms } else { modified }).saturating_sub(started) / 1000,
            "tool_calls": tools,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        });
        if let Some(matched) = record.get("matched") {
            row["matched"] = matched.clone();
        }
        rows.push((started, row));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// One line saying what a job did last: its latest words, else its latest
/// command, else how it ended.
fn summarise(steps: &[Value]) -> String {
    let line = |text: &str| text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    for step in steps.iter().rev() {
        match step["kind"].as_str() {
            Some("text") => return line(step["text"].as_str().unwrap_or("")),
            Some("tool") => return format!("$ {}", line(step["command"].as_str().unwrap_or(""))),
            Some("error") => return line(step["text"].as_str().unwrap_or("error")),
            Some("verify") => return format!("verify {}", step["status"].as_str().unwrap_or("")),
            _ => {}
        }
    }
    String::new()
}

/// Longest tool output kept per step in a job timeline.
const TIMELINE_OUTPUT_CHARS: usize = 4_000;

/// One job's record plus a readable timeline of what its child did: the
/// prompt, the agent's text, each command with its result, verification and
/// the ending. Built from the child's stream-json log so clients never parse
/// it themselves. `None` when no record has that id.
pub(crate) fn detail(jobs_dir: &Path, job_id: &str, now_ms: u128) -> Option<Value> {
    let row = list(jobs_dir, now_ms, u128::MAX)
        .into_iter()
        .find(|row| row["job_id"] == job_id)?;
    let record: Value = fs::read_to_string(jobs_dir.join(format!("{job_id}.json")))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())?;
    let log = record["output"].as_str().map(Path::new);
    let timeline = log.map(timeline).unwrap_or_default();
    Some(json!({"job": row, "failed_claims": record["failed_claims"], "timeline": timeline}))
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}\n[… {} more characters]", text.chars().count() - max)
}

fn timeline(log: &Path) -> Vec<Value> {
    let Ok(text) = fs::read_to_string(log) else {
        return Vec::new();
    };
    let mut steps: Vec<Value> = Vec::new();
    let mut buffer = String::new();
    // Tagged blocks ([VERIFY], [SPEAK], [SOURCES]) are machine records with
    // their own steps; the text step keeps only what the agent said.
    let flush = |buffer: &mut String, steps: &mut Vec<Value>| {
        let said = crate::core::answer::strip(buffer, &["VERIFY", "SPEAK", "SOURCES"]);
        if !said.is_empty() {
            steps.push(json!({"kind": "text", "text": said}));
        }
        buffer.clear();
    };
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
        match event["type"].as_str() {
            Some("invoke_start") => {
                if let Some(prompt) = event["prompt"].as_str() {
                    steps.push(json!({"kind": "prompt", "text": prompt}));
                }
            }
            Some("text_delta") => buffer.push_str(event["text"].as_str().unwrap_or("")),
            Some("text_done") => flush(&mut buffer, &mut steps),
            Some("tool_use") => {
                flush(&mut buffer, &mut steps);
                steps.push(json!({
                    "kind": "tool",
                    "id": event["id"],
                    "command": event["input"]["command"],
                }));
            }
            Some("tool_result") => {
                let id = &event["tool_use_id"];
                if let Some(step) = steps.iter_mut().rev().find(|s| s["kind"] == "tool" && &s["id"] == id) {
                    step["exit_code"] = event["exit_code"].clone();
                    step["output"] = json!(clip(event["content"].as_str().unwrap_or(""), TIMELINE_OUTPUT_CHARS));
                }
            }
            Some("verify") => steps.push(json!({
                "kind": "verify",
                "status": event["status"],
                "claims": event["claims"],
            })),
            Some("invoke_end") => {
                flush(&mut buffer, &mut steps);
                steps.push(json!({
                    "kind": "end",
                    "stop_reason": event["stop_reason"],
                    "duration_ms": event["duration_ms"],
                    "input_tokens": event["input_tokens"],
                    "output_tokens": event["output_tokens"],
                }));
            }
            Some("error") => {
                flush(&mut buffer, &mut steps);
                steps.push(json!({"kind": "error", "text": event["message"]}));
            }
            _ => {}
        }
    }
    flush(&mut buffer, &mut steps);
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_log_becomes_a_readable_timeline() {
        let dir = std::env::temp_dir().join(format!("lazar-timeline-{}-{}", std::process::id(), crate::now_millis()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("out.log");
        let lines = [
            json!({"type":"invoke_start","prompt":"write DONE"}),
            json!({"type":"text_delta","index":1,"text":"\n\n"}),
            json!({"type":"text_done","index":1}),
            json!({"type":"tool_use","id":"t1","input":{"command":"echo DONE > /tmp/x"}}),
            json!({"type":"tool_result","tool_use_id":"t1","content":"[exit 0]","exit_code":0}),
            json!({"type":"text_delta","index":1,"text":"All "}),
            json!({"type":"text_delta","index":1,"text":"done.\n[VERIFY]\nexists=/tmp/x\n[/VERIFY]"}),
            json!({"type":"text_done","index":1}),
            json!({"type":"verify","status":"passed","claims":[{"claim":"exists=/tmp/x","ok":true}]}),
            json!({"type":"invoke_end","stop_reason":"end_turn","duration_ms":6220}),
        ];
        fs::write(&log, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
        let steps = timeline(&log);
        let kinds: Vec<&str> = steps.iter().map(|s| s["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["prompt", "tool", "text", "verify", "end"], "whitespace-only text is dropped");
        assert_eq!(steps[1]["command"], "echo DONE > /tmp/x");
        assert_eq!(steps[1]["exit_code"], 0);
        assert_eq!(steps[2]["text"], "All done.");
        assert_eq!(summarise(&steps), "verify passed");
        assert_eq!(summarise(&steps[..3]), "All done.");
        assert_eq!(summarise(&steps[..2]), "$ echo DONE > /tmp/x");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_finished_and_lost_jobs_are_reported() {
        let dir = std::env::temp_dir().join(format!("lazar-jobs-{}-{}", std::process::id(), crate::now_millis()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("out.log");
        fs::write(
            &log,
            "{\"type\":\"tool_use\"}\n{\"type\":\"context\",\"input_tokens\":1200,\"output_tokens\":40}\n{\"type\":\"tool_use\"}\n{\"type\":\"context\",\"input_tokens\":1500,\"output_tokens\":60}\n",
        )
        .unwrap();
        let now = crate::now_millis();
        let me = std::process::id();
        fs::write(dir.join("a.json"), json!({"status":"running","job_id":"a","task":"alpha","pid":me,"output":log,"started_ms":(now - 15_000) as u64}).to_string()).unwrap();
        fs::write(dir.join("b.json"), json!({"status":"ok","job_id":"b","task":"beta","matched":true}).to_string()).unwrap();
        fs::write(dir.join("c.json"), json!({"status":"running","job_id":"c","task":"gamma","pid":999_999_999i64}).to_string()).unwrap();
        fs::write(dir.join("a.spec.json"), "{}").unwrap();

        let rows = list(&dir, now, RECENT_MS);
        assert_eq!(rows.len(), 3, "{rows:?}");
        let a = rows.iter().find(|r| r["job_id"] == "a").unwrap();
        assert_eq!(a["status"], "running");
        assert_eq!(a["tool_calls"], 2);
        assert_eq!(a["input_tokens"], 1500);
        assert_eq!(a["output_tokens"], 100);
        assert!(a["elapsed_s"].as_u64().unwrap() >= 14);
        assert_eq!(rows.iter().find(|r| r["job_id"] == "c").unwrap()["status"], "lost");
        assert_eq!(rows.iter().find(|r| r["job_id"] == "b").unwrap()["matched"], true);

        let shown = detail(&dir, "a", now).unwrap();
        assert_eq!(shown["job"]["task"], "alpha");
        assert!(detail(&dir, "nope", now).is_none());

        // Finished long ago: dropped.
        assert_eq!(list(&dir, now + RECENT_MS * 2, RECENT_MS).len(), 1, "only the live job remains");
        let _ = fs::remove_dir_all(&dir);
    }
}
