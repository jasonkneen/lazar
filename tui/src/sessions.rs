//! Lazar sessions (other lazartui windows, ACP clients) shown on the Agents
//! screen next to spawned jobs. Read straight from the kernel's session logs
//! (`$LAZAR_HOME/user/logs/sessions/<id>.jsonl`) — nothing here writes them.
//! Rows reuse `jobs::Job` with a `session:` id prefix so the list, detail
//! and steer paths stay shared.

use crate::jobs::{Job, SendOutcome};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const PREFIX: &str = "session:";
/// Sessions touched within this window are listed.
const MAX_AGE: Duration = Duration::from_secs(24 * 3_600);
/// A turn with no log activity for this long is treated as dead, not running.
const STALE: Duration = Duration::from_secs(15 * 60);
/// Only the end of a log is read: enough for status and a recent timeline.
const TAIL_BYTES: u64 = 512 * 1024;

pub fn sessions_dir(lazar_home: &Path) -> PathBuf {
    lazar_home.join("user").join("logs").join("sessions")
}

pub fn session_id_of(job_id: &str) -> Option<&str> {
    job_id.strip_prefix(PREFIX)
}

fn read_tail(path: &Path) -> Vec<Value> {
    let Ok(mut file) = fs::File::open(path) else { return Vec::new() };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let skip_partial = len > TAIL_BYTES;
    if skip_partial {
        let _ = file.seek(SeekFrom::Start(len - TAIL_BYTES));
    }
    let mut text = String::new();
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    text.push_str(&String::from_utf8_lossy(&bytes));
    let mut lines = text.lines();
    if skip_partial {
        lines.next();
    }
    lines.filter_map(|line| serde_json::from_str(line).ok()).collect()
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect()
}

/// One Agents-screen row for a session log.
pub fn session_row(id: &str, events: &[Value], age: Duration) -> Job {
    let mut running = false;
    let mut last_stop = String::new();
    let mut last_prompt = String::new();
    let mut last_text = String::new();
    let mut tool_calls = 0;
    for event in events {
        match event["kind"].as_str().unwrap_or("") {
            "invoke_start" => running = true,
            "invoke_end" => {
                running = false;
                last_stop = event["stop_reason"].as_str().unwrap_or("").to_string();
            }
            "user" => last_prompt = one_line(&text_of(&event["content"])),
            "assistant" => {
                let text = one_line(&text_of(&event["content"]));
                if !text.is_empty() {
                    last_text = text;
                }
                tool_calls += event["content"]
                    .as_array()
                    .map(|b| b.iter().filter(|b| b["type"] == "tool_use").count() as u64)
                    .unwrap_or(0);
            }
            _ => {}
        }
    }
    let status = if running && age < STALE {
        "running"
    } else if running {
        "stalled"
    } else if matches!(last_stop.as_str(), "end_turn" | "stop_sequence" | "") {
        "ok"
    } else {
        last_stop.as_str()
    };
    Job {
        job_id: format!("{PREFIX}{id}"),
        task: if last_prompt.is_empty() { id.to_string() } else { last_prompt },
        status: status.to_string(),
        tool_calls,
        summary: last_text,
        age_s: age.as_secs(),
        // Only a live turn can be steered (through its inbox).
        session: (status == "running").then(|| id.to_string()),
        ..Job::default()
    }
}

/// Sessions active in the last day, newest first. `main` is this window's
/// own session, labelled and pinned first; `exclude` holds ids already shown
/// as spawned jobs.
pub fn scan(lazar_home: &Path, main: Option<&str>, exclude: &[String], hidden: &[String]) -> Vec<Job> {
    let now = SystemTime::now();
    let mut found: Vec<(Duration, Job)> = Vec::new();
    let Ok(entries) = fs::read_dir(sessions_dir(lazar_home)) else { return Vec::new() };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
        let is_main = main == Some(id.as_str());
        if !is_main && (exclude.contains(&id) || hidden.contains(&format!("{PREFIX}{id}"))) {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .unwrap_or_default();
        if age > MAX_AGE && !is_main {
            continue;
        }
        let mut row = session_row(&id, &read_tail(&path), age);
        if is_main {
            row.task = format!("main · {}", row.task);
            found.insert(0, (Duration::ZERO, row));
        } else {
            found.push((age, row));
        }
    }
    let main_row = main.and_then(|m| {
        found
            .iter()
            .position(|(_, j)| j.job_id == format!("{PREFIX}{m}"))
            .map(|i| found.remove(i).1)
    });
    found.sort_by_key(|(age, _)| *age);
    main_row.into_iter().chain(found.into_iter().map(|(_, j)| j)).collect()
}

/// The `jobs --show` shape (`{"job":{..},"timeline":[..]}`) built from a
/// session log, so the job-detail screen renders it unchanged.
pub fn show(lazar_home: &Path, id: &str) -> Result<Value, String> {
    let path = sessions_dir(lazar_home).join(format!("{id}.jsonl"));
    let age = fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .unwrap_or_default();
    let events = read_tail(&path);
    if events.is_empty() {
        return Err(format!("cannot read {}", path.display()));
    }
    let row = session_row(id, &events, age);
    let mut timeline: Vec<Value> = Vec::new();
    let mut tool_index: Vec<(String, usize)> = Vec::new();
    for event in &events {
        match event["kind"].as_str().unwrap_or("") {
            "user" | "steer" => {
                let text = text_of(&event["content"]);
                if !text.is_empty() {
                    timeline.push(json!({"kind": "operator", "text": text, "steer": event["kind"] == "steer"}));
                }
            }
            "assistant" => {
                for block in event["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") if block["text"].as_str().is_some_and(|t| !t.trim().is_empty()) => {
                            timeline.push(json!({"kind": "text", "text": block["text"]}));
                        }
                        Some("tool_use") => {
                            let tool_id = block["id"].as_str().unwrap_or("").to_string();
                            tool_index.push((tool_id.clone(), timeline.len()));
                            timeline.push(json!({"kind": "tool", "id": tool_id, "command": block["input"]["command"]}));
                        }
                        _ => {}
                    }
                }
            }
            "tool_result" => {
                let tool_id = event["tool_use_id"].as_str().unwrap_or("");
                if let Some((_, at)) = tool_index.iter().find(|(t, _)| t == tool_id) {
                    timeline[*at]["output"] = event["content"].clone();
                    timeline[*at]["exit_code"] = event["exit_code"].clone();
                }
            }
            _ => {}
        }
    }
    Ok(json!({
        "job": {"status": row.status, "task": row.task, "session": row.session},
        "timeline": timeline,
    }))
}

/// Steer a running session: the kernel drains `<id>.inbox` before its next
/// model request.
pub fn steer(lazar_home: &Path, id: &str, text: &str) -> Result<SendOutcome, String> {
    let path = sessions_dir(lazar_home).join(format!("{id}.inbox"));
    let line = json!({ "text": text }).to_string();
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{line}"))
        .map_err(|e| format!("cannot steer {}: {e}", path.display()))?;
    Ok(SendOutcome { job_id: format!("{PREFIX}{id}"), delivered: "steer".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("lazartui-sessions-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(sessions_dir(&root)).unwrap();
        root
    }

    fn write(root: &Path, id: &str, events: &[Value]) {
        let body: String = events.iter().map(|e| e.to_string() + "\n").collect();
        fs::write(sessions_dir(root).join(format!("{id}.jsonl")), body).unwrap();
    }

    #[test]
    fn lists_main_first_marks_running_and_honours_hidden_and_excluded() {
        let root = home("scan");
        write(&root, "mine", &[json!({"kind":"user","content":"my task"})]);
        write(&root, "other", &[
            json!({"kind":"invoke_start"}),
            json!({"kind":"user","content":"other window work"}),
            json!({"kind":"assistant","content":[{"type":"text","text":"checking"},{"type":"tool_use","id":"a","input":{"command":"ls"}}]}),
        ]);
        write(&root, "hidden", &[json!({"kind":"user","content":"x"})]);
        write(&root, "jobsess", &[json!({"kind":"user","content":"y"})]);
        let rows = scan(&root, Some("mine"), &["jobsess".into()], &["session:hidden".into()]);
        let ids: Vec<_> = rows.iter().map(|r| r.job_id.as_str()).collect();
        assert_eq!(ids, vec!["session:mine", "session:other"]);
        assert!(rows[0].task.starts_with("main · my task"));
        assert_eq!(rows[1].status, "running");
        assert_eq!(rows[1].session.as_deref(), Some("other"));
        assert_eq!(rows[1].summary, "checking");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn show_builds_a_timeline_and_steer_writes_the_inbox() {
        let root = home("show");
        write(&root, "s", &[
            json!({"kind":"user","content":"do it"}),
            json!({"kind":"assistant","content":[{"type":"tool_use","id":"t","input":{"command":"ls"}}]}),
            json!({"kind":"tool_result","tool_use_id":"t","content":"a\nb","exit_code":0}),
            json!({"kind":"assistant","content":[{"type":"text","text":"done"}]}),
            json!({"kind":"invoke_end","stop_reason":"end_turn"}),
        ]);
        let shown = show(&root, "s").unwrap();
        let kinds: Vec<_> = shown["timeline"].as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["operator", "tool", "text"]);
        assert_eq!(shown["timeline"][1]["output"], "a\nb");
        assert_eq!(shown["job"]["status"], "ok");
        steer(&root, "s", "hurry").unwrap();
        let inbox = fs::read_to_string(sessions_dir(&root).join("s.inbox")).unwrap();
        assert!(inbox.contains("hurry"));
        let _ = fs::remove_dir_all(&root);
    }
}
