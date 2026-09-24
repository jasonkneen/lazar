//! Background sub-agent jobs, polled from the kernel (`<lazar> jobs`) so the
//! TUI never tracks agent state itself — it only displays what the kernel
//! reports. Polling runs on its own thread so the UI never blocks on the
//! subprocess.

use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn default_status() -> String {
    "unknown".into()
}

/// One row of `<lazar> jobs`' JSON array. Every field but the id is
/// defaulted so an older or partial kernel response still parses.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct Job {
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub task: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub elapsed_s: u64,
    #[serde(default)]
    pub tool_calls: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub matched: bool,
    /// A one-line summary of the job's latest activity, for the Agents
    /// screen's row (the footer panel doesn't show it — no room).
    #[serde(default)]
    pub summary: String,
    /// Seconds since the kernel's record for this job last changed — the
    /// Agents screen's right-aligned age column.
    #[serde(default)]
    pub age_s: u64,
    /// The kernel's session id for this job, present only when it can take
    /// a follow-up message (`jobs --send`) — absent/null for older jobs
    /// with no session left to continue.
    #[serde(default)]
    pub session: Option<String>,
}

impl Job {
    pub fn is_running(&self) -> bool {
        matches!(self.status.as_str(), "running" | "starting")
    }

    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }

    /// Which of the Agents screen's three sections this job belongs in.
    /// Anything that isn't running/starting or ok fails closed into
    /// "needs attention" — same rule as `status::agent_rows`' glyph choice.
    pub fn category(&self) -> JobCategory {
        if self.is_running() {
            JobCategory::Working
        } else if self.is_ok() {
            JobCategory::Completed
        } else {
            JobCategory::Attention
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCategory {
    Working,
    Attention,
    Completed,
}

/// Jobs bucketed into the Agents screen's three sections — Working, Needs
/// attention, Completed — each preserving the kernel's own row order. The
/// same order is used for the flat up/down selection index, so a section
/// header never gets a selection slot.
pub fn group_jobs(jobs: &[Job]) -> (Vec<&Job>, Vec<&Job>, Vec<&Job>) {
    let mut working = Vec::new();
    let mut attention = Vec::new();
    let mut completed = Vec::new();
    for job in jobs {
        match job.category() {
            JobCategory::Working => working.push(job),
            JobCategory::Attention => attention.push(job),
            JobCategory::Completed => completed.push(job),
        }
    }
    (working, attention, completed)
}

/// The Agents screen's row order, flattened: Working, then Needs attention,
/// then Completed. Used both to size the selection range and to resolve a
/// selected index back to a job.
pub fn selectable_jobs(jobs: &[Job]) -> Vec<&Job> {
    let (working, attention, completed) = group_jobs(jobs);
    working.into_iter().chain(attention).chain(completed).collect()
}

/// "12s" / "5m" / "2h" / "1d" — the Agents screen's age column, from the
/// kernel's `age_s`.
pub fn format_age(age_s: u64) -> String {
    if age_s < 60 {
        format!("{age_s}s")
    } else if age_s < 3_600 {
        format!("{}m", age_s / 60)
    } else if age_s < 86_400 {
        format!("{}h", age_s / 3_600)
    } else {
        format!("{}d", age_s / 86_400)
    }
}

pub fn parse_jobs(json: &str) -> Result<Vec<Job>, String> {
    serde_json::from_str(json).map_err(|e| format!("bad jobs JSON: {e}"))
}

/// Polls `<lazar> jobs` on a background thread, at `interval`, and hands each
/// successful parse to the UI thread through a channel.
pub struct JobsHandle {
    events: Receiver<Vec<Job>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl JobsHandle {
    pub fn spawn(kernel: PathBuf, lazar_home: PathBuf) -> Self {
        Self::spawn_with_interval(kernel, lazar_home, Duration::from_secs(2))
    }

    pub fn spawn_with_interval(kernel: PathBuf, lazar_home: PathBuf, interval: Duration) -> Self {
        Self::spawn_with_args(kernel, lazar_home, interval, Vec::new())
    }

    /// Like `spawn`, but with extra CLI args appended after `jobs` — e.g.
    /// `["--since", "86400"]` for the full Agents screen, vs. the footer
    /// panel's default (no args, the kernel's own last-10-minutes window).
    pub fn spawn_with_args(
        kernel: PathBuf,
        lazar_home: PathBuf,
        interval: Duration,
        args: Vec<String>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let stop_worker = stop.clone();
        let worker = thread::Builder::new()
            .name("lazartui-jobs".into())
            .spawn(move || loop {
                if stop_worker.load(Ordering::Relaxed) {
                    return;
                }
                if let Ok(output) = Command::new(&kernel)
                    .arg("jobs")
                    .args(&args)
                    .env("LAZAR_HOME", &lazar_home)
                    .output()
                {
                    if output.status.success() {
                        if let Ok(jobs) = parse_jobs(&String::from_utf8_lossy(&output.stdout)) {
                            if tx.send(jobs).is_err() {
                                return;
                            }
                        }
                    }
                }
                // Sleep in small steps so `stop` (dropping the handle) is
                // responsive instead of blocking shutdown for the interval.
                let step = Duration::from_millis(50);
                let mut waited = Duration::ZERO;
                while waited < interval {
                    if stop_worker.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(step.min(interval - waited));
                    waited += step;
                }
            })
            .expect("spawn jobs poll thread");
        Self {
            events: rx,
            stop,
            worker: Some(worker),
        }
    }

    /// Drains the channel and returns the most recent poll, if any arrived
    /// since the last call.
    pub fn try_recv_latest(&self) -> Option<Vec<Job>> {
        let mut latest = None;
        while let Ok(jobs) = self.events.try_recv() {
            latest = Some(jobs);
        }
        latest
    }
}

impl Drop for JobsHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// One-shot `<lazar> jobs --show <job_id>` off the UI thread: the job
/// detail screen's `{"job": ..., "failed_claims": ..., "timeline": [...]}`.
/// The thread sends exactly one result and exits; a dropped receiver (the
/// detail view closed, or a fresher fetch replaced it) just makes the send
/// fail silently.
pub fn fetch_job_show(
    kernel: PathBuf,
    lazar_home: PathBuf,
    job_id: String,
) -> Receiver<Result<Value, String>> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("lazartui-job-show".into())
        .spawn(move || {
            if let Some(id) = crate::sessions::session_id_of(&job_id) {
                let _ = tx.send(crate::sessions::show(&lazar_home, id));
                return;
            }
            let outcome = match Command::new(&kernel)
                .arg("jobs")
                .arg("--show")
                .arg(&job_id)
                .env("LAZAR_HOME", &lazar_home)
                .output()
            {
                Ok(output) if output.status.success() => {
                    serde_json::from_slice::<Value>(&output.stdout)
                        .map_err(|e| format!("bad job JSON: {e}"))
                }
                Ok(output) => Err(format!(
                    "jobs --show {job_id} failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(e) => Err(format!("cannot run {}: {e}", kernel.display())),
            };
            let _ = tx.send(outcome);
        })
        .expect("spawn job-show thread");
    rx
}

/// The kernel's `jobs --send` result: which delivery path it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    pub job_id: String,
    /// "steer" (queued into the running job, ahead of its next model
    /// request) or "turn" (a finished job's session got a fresh follow-up
    /// turn).
    pub delivered: String,
}

/// One-shot `lazar jobs --start "<task>"` off the UI thread: the new job's
/// initial record, so the Agents screen can show or select it right away
/// instead of waiting for the next 2s poll.
pub fn start_job(kernel: PathBuf, lazar_home: PathBuf, task: String) -> Receiver<Result<Job, String>> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("lazartui-job-start".into())
        .spawn(move || {
            let outcome = match Command::new(&kernel)
                .arg("jobs")
                .arg("--start")
                .arg(&task)
                .env("LAZAR_HOME", &lazar_home)
                .output()
            {
                Ok(output) if output.status.success() => {
                    serde_json::from_slice::<Job>(&output.stdout).map_err(|e| format!("bad job JSON: {e}"))
                }
                Ok(output) => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
                Err(e) => Err(format!("cannot run {}: {e}", kernel.display())),
            };
            let _ = tx.send(outcome);
        })
        .expect("spawn job-start thread");
    rx
}

/// One-shot `lazar jobs --send <id> "<text>"` off the UI thread: steers a
/// running job, or starts a follow-up turn on a finished one.
pub fn send_job(
    kernel: PathBuf,
    lazar_home: PathBuf,
    job_id: String,
    text: String,
) -> Receiver<Result<SendOutcome, String>> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("lazartui-job-send".into())
        .spawn(move || {
            if let Some(id) = crate::sessions::session_id_of(&job_id) {
                let _ = tx.send(crate::sessions::steer(&lazar_home, id, &text));
                return;
            }
            let outcome = match Command::new(&kernel)
                .arg("jobs")
                .arg("--send")
                .arg(&job_id)
                .arg(&text)
                .env("LAZAR_HOME", &lazar_home)
                .output()
            {
                Ok(output) if output.status.success() => serde_json::from_slice::<Value>(&output.stdout)
                    .map_err(|e| format!("bad send JSON: {e}"))
                    .map(|v| SendOutcome {
                        job_id: v.get("job_id").and_then(Value::as_str).unwrap_or(&job_id).to_string(),
                        delivered: v.get("delivered").and_then(Value::as_str).unwrap_or("").to_string(),
                    }),
                Ok(output) => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
                Err(e) => Err(format!("cannot run {}: {e}", kernel.display())),
            };
            let _ = tx.send(outcome);
        })
        .expect("spawn job-send thread");
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn job(id: &str, status: &str) -> Job {
        Job {
            job_id: id.into(),
            status: status.into(),
            ..Job::default()
        }
    }

    #[test]
    fn group_jobs_buckets_by_category_and_keeps_kernel_order() {
        let jobs = vec![
            job("a", "running"),
            job("b", "fail"),
            job("c", "ok"),
            job("d", "starting"),
            job("e", "lost"),
            job("f", "unknown"),
        ];
        let (working, attention, completed) = group_jobs(&jobs);
        assert_eq!(working.iter().map(|j| j.job_id.as_str()).collect::<Vec<_>>(), ["a", "d"]);
        assert_eq!(
            attention.iter().map(|j| j.job_id.as_str()).collect::<Vec<_>>(),
            ["b", "e", "f"],
            "anything that isn't running/starting/ok fails closed into attention"
        );
        assert_eq!(completed.iter().map(|j| j.job_id.as_str()).collect::<Vec<_>>(), ["c"]);

        let flat = selectable_jobs(&jobs);
        assert_eq!(
            flat.iter().map(|j| j.job_id.as_str()).collect::<Vec<_>>(),
            ["a", "d", "b", "e", "f", "c"],
            "selection order is working, then attention, then completed"
        );

        let (working, attention, completed) = group_jobs(&[]);
        assert!(working.is_empty() && attention.is_empty() && completed.is_empty());
    }

    #[test]
    fn format_age_uses_the_largest_whole_unit() {
        assert_eq!(format_age(0), "0s");
        assert_eq!(format_age(59), "59s");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(3_599), "59m");
        assert_eq!(format_age(3_600), "1h");
        assert_eq!(format_age(86_399), "23h");
        assert_eq!(format_age(86_400), "1d");
        assert_eq!(format_age(200_000), "2d");
    }

    #[test]
    fn parses_jobs_tolerating_unknown_and_missing_fields() {
        let jobs = parse_jobs(
            r#"[
                {"job_id":"1","task":"audit repo","status":"running","elapsed_s":12,"tool_calls":3,"input_tokens":4000,"output_tokens":100,"matched":true,"weird_extra_field":true},
                {"job_id":"2","task":"lint"}
            ]"#,
        )
        .unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].status, "running");
        assert!(jobs[0].is_running());
        assert_eq!(jobs[1].status, "unknown", "missing status defaults instead of failing to parse");
        assert_eq!(jobs[1].elapsed_s, 0);
        assert!(parse_jobs("not json").is_err());
    }

    fn write_fake_jobs_kernel(root: &std::path::Path, json: &str) -> PathBuf {
        let script = root.join("fake-lazar-jobs");
        fs::write(&script, format!("#!/usr/bin/env bash\ncat <<'EOF'\n{json}\nEOF\n")).unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        script
    }

    #[test]
    fn polls_the_kernel_and_delivers_parsed_jobs() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-jobs-poll-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let binary = write_fake_jobs_kernel(
            &root,
            r#"[{"job_id":"1","task":"audit","status":"ok","elapsed_s":3,"tool_calls":1,"input_tokens":500,"output_tokens":50,"matched":true}]"#,
        );
        let handle = JobsHandle::spawn_with_interval(binary, root.clone(), Duration::from_millis(20));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let jobs = loop {
            assert!(std::time::Instant::now() < deadline, "jobs poll never delivered a result");
            if let Some(jobs) = handle.try_recv_latest() {
                if !jobs.is_empty() {
                    break jobs;
                }
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(jobs[0].job_id, "1");
        assert_eq!(jobs[0].task, "audit");
        assert!(jobs[0].is_ok());
        drop(handle);
        fs::remove_dir_all(root).unwrap();
    }
}
