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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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

// ─── Operator jobs and steering ──────────────────────────────────────────
//
// `jobs --start` runs a task as a detached agent with its own session;
// `jobs --send` steers a running job through its inbox (the child's loop
// drains `LAZAR_INBOX` before every model request) or, once it has finished,
// continues it with a new turn in the same session. `jobs --run` is the
// detached runner that owns the child and records how it ended.

/// Inbox a running job's child drains before each model request.
pub(crate) fn inbox_path(jobs_dir: &Path, job_id: &str) -> PathBuf {
    jobs_dir.join(format!("{job_id}.inbox"))
}

fn record_path(jobs_dir: &Path, job_id: &str) -> PathBuf {
    jobs_dir.join(format!("{job_id}.json"))
}

fn read_record(jobs_dir: &Path, job_id: &str) -> Result<Value, String> {
    crate::session::validate_session_id(job_id).map_err(|e| format!("bad job id: {e}"))?;
    let text = fs::read_to_string(record_path(jobs_dir, job_id))
        .map_err(|_| format!("no job with id {job_id}"))?;
    serde_json::from_str(&text).map_err(|e| format!("job {job_id} record is unreadable: {e}"))
}

fn write_record(jobs_dir: &Path, job_id: &str, record: &Value) -> Result<(), String> {
    fs::create_dir_all(jobs_dir).map_err(|e| e.to_string())?;
    let path = record_path(jobs_dir, job_id);
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&tmp, format!("{record}\n")).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// `<slug of the task>-<ms>-<pid>`: a valid session id, at most 64 chars.
pub(crate) fn new_job_id(task: &str, now_ms: u128) -> String {
    let slug: String = task
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').chars().take(30).collect::<String>();
    let slug = if slug.is_empty() { "task".to_string() } else { slug };
    format!("{slug}-{now_ms}-{}", std::process::id())
}

fn title(task: &str) -> String {
    let line = task.lines().find(|l| !l.trim().is_empty()).unwrap_or("task").trim();
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(79).collect::<String>())
    } else {
        line.to_string()
    }
}

/// Start `exe jobs --run <id>` in its own session, detached from the caller.
fn spawn_runner(exe: &Path, job_id: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(exe);
    command
        .args(["jobs", "--run", job_id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid only detaches the runner from the caller's terminal and
    // process group; it touches no memory shared with the parent.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn().map(|_| ()).map_err(|e| format!("cannot start job runner: {e}"))
}

/// Create an operator job for `task` and start it. Returns the new record.
pub(crate) fn start(jobs_dir: &Path, task: &str, exe: &Path, now_ms: u128) -> Result<Value, String> {
    if task.trim().is_empty() {
        return Err("empty task".into());
    }
    let job_id = new_job_id(task, now_ms);
    let output = jobs_dir
        .parent()
        .unwrap_or(jobs_dir)
        .join(format!("out-{job_id}.log"));
    let record = json!({
        "status": "starting",
        "job_id": job_id,
        "task": title(task),
        "kind": "operator",
        "prompt": task,
        "session": job_id,
        "output": output,
        "started_ms": now_ms as u64,
    });
    write_record(jobs_dir, &job_id, &record)?;
    spawn_runner(exe, &job_id)?;
    Ok(record)
}

/// Runner body: run the job's pending prompt as one turn of its session,
/// then record the outcome. Output is appended, so follow-up turns extend
/// the same timeline.
pub(crate) fn run(jobs_dir: &Path, job_id: &str, exe: &Path) -> Result<(), String> {
    let mut record = read_record(jobs_dir, job_id)?;
    let prompt = record["prompt"].as_str().unwrap_or("").to_string();
    let session = record["session"].as_str().unwrap_or(job_id).to_string();
    let output = PathBuf::from(record["output"].as_str().ok_or("job has no output log")?);
    record["status"] = json!("running");
    record["pid"] = json!(std::process::id());
    write_record(jobs_dir, job_id, &record)?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&output)
        .map_err(|e| format!("cannot open {}: {e}", output.display()))?;
    let status = Command::new(exe)
        .args(["-p", &prompt, "--session", &session, "--output-format", "stream-json"])
        .env("LAZAR_INBOX", inbox_path(jobs_dir, job_id))
        .env_remove("LAZAR_TOOL_ENV")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .status();
    let rc = status.as_ref().ok().and_then(|s| s.code()).unwrap_or(1);
    let mut record = read_record(jobs_dir, job_id).unwrap_or(record);
    record["status"] = json!(if rc == 0 { "ok" } else { "fail" });
    record["child_rc"] = json!(rc);
    write_record(jobs_dir, job_id, &record)
}

/// Steer a running job (queued in its inbox for the next model request) or
/// continue a finished one with a new turn in its session.
pub(crate) fn send(jobs_dir: &Path, job_id: &str, text: &str, exe: &Path, now_ms: u128) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Err("empty message".into());
    }
    let mut record = read_record(jobs_dir, job_id)?;
    let running = matches!(record["status"].as_str(), Some("running" | "starting"))
        && record["pid"].as_i64().is_none_or(alive);
    if running {
        let mut inbox = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(inbox_path(jobs_dir, job_id))
            .map_err(|e| e.to_string())?;
        writeln!(inbox, "{}", json!({"text": text})).map_err(|e| e.to_string())?;
        return Ok(json!({"job_id": job_id, "delivered": "steer"}));
    }
    if record.get("session").is_none() {
        return Err(format!("job {job_id} has no session to continue"));
    }
    record["prompt"] = json!(text);
    record["status"] = json!("starting");
    record["started_ms"] = json!(now_ms as u64);
    record.as_object_mut().map(|o| {
        o.remove("pid");
        o.remove("child_rc");
        o.remove("matched");
        o.remove("failed_claims");
    });
    write_record(jobs_dir, job_id, &record)?;
    spawn_runner(exe, job_id)?;
    Ok(json!({"job_id": job_id, "delivered": "turn"}))
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
        let said = crate::core::answer::display(buffer).trim().to_string();
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
                    flush(&mut buffer, &mut steps);
                    // The first prompt is the task; later ones are the
                    // operator continuing the job.
                    let kind = if steps.iter().any(|s| s["kind"] == "prompt") { "operator" } else { "prompt" };
                    steps.push(json!({"kind": kind, "text": prompt}));
                }
            }
            Some("steer") => {
                flush(&mut buffer, &mut steps);
                steps.push(json!({"kind": "operator", "text": event["text"], "steer": true}));
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
            json!({"type":"invoke_start","prompt":"now add tests"}),
            json!({"type":"steer","text":"only unit tests"}),
        ];
        fs::write(&log, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
        let steps = timeline(&log);
        let kinds: Vec<&str> = steps.iter().map(|s| s["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            ["prompt", "tool", "text", "verify", "end", "operator", "operator"],
            "whitespace-only text is dropped; later prompts and steering are the operator's"
        );
        assert_eq!(steps[1]["command"], "echo DONE > /tmp/x");
        assert_eq!(steps[1]["exit_code"], 0);
        assert_eq!(steps[2]["text"], "All done.");
        assert_eq!(summarise(&steps[..5]), "verify passed");
        assert_eq!(summarise(&steps[..3]), "All done.");
        assert_eq!(summarise(&steps[..2]), "$ echo DONE > /tmp/x");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn job_ids_are_valid_session_ids() {
        for task in ["Refactor the TUI footer!", "", "   ", "ünïcödé task with a very long description indeed"] {
            let id = new_job_id(task, 1_790_000_000_000);
            crate::session::validate_session_id(&id).unwrap();
            assert!(id.len() <= 64, "{id}");
        }
        assert!(new_job_id("Refactor the TUI footer!", 1).starts_with("refactor-the-tui-footer"));
    }

    #[test]
    fn send_steers_a_running_job_and_continues_a_finished_one() {
        let dir = std::env::temp_dir().join(format!("lazar-send-{}-{}", std::process::id(), crate::now_millis()));
        fs::create_dir_all(&dir).unwrap();
        let exe = Path::new("/usr/bin/true");
        let me = std::process::id();
        write_record(&dir, "live", &json!({"status":"running","job_id":"live","pid":me,"session":"live","output":dir.join("o.log")})).unwrap();
        let steered = send(&dir, "live", "focus on the footer", exe, 1).unwrap();
        assert_eq!(steered["delivered"], "steer");
        let inbox = fs::read_to_string(inbox_path(&dir, "live")).unwrap();
        assert!(inbox.contains("focus on the footer"), "{inbox}");

        write_record(&dir, "done", &json!({"status":"ok","job_id":"done","pid":999_999_999i64,"session":"done","output":dir.join("o.log"),"child_rc":0})).unwrap();
        let continued = send(&dir, "done", "now add tests", exe, 5).unwrap();
        assert_eq!(continued["delivered"], "turn");
        let record = read_record(&dir, "done").unwrap();
        assert_eq!(record["prompt"], "now add tests");
        assert_eq!(record["status"], "starting");
        assert!(record.get("child_rc").is_none());

        write_record(&dir, "spawned", &json!({"status":"fail","job_id":"spawned"})).unwrap();
        assert!(send(&dir, "spawned", "retry", exe, 5).unwrap_err().contains("no session"));
        assert!(send(&dir, "../etc", "x", exe, 5).is_err(), "ids are validated");
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
