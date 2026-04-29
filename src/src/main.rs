//! lazar — the smallest self-evolving agent harness.
//!
//! One tool: `execute(command)` runs bash through sandbox-exec.
//! Everything else lives as skills under ~/lazar/skills/.
//! Seed skills are embedded in the binary so `--reset-all` is a
//! true factory restore.

use clap::{Parser, ValueEnum};
use include_dir::{include_dir, Dir};
use serde_json::{json, Value};
use std::{
    env, fs,
    fs::File,
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd, process::CommandExt};

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFormat {
    /// Human-readable: streams assistant text to stdout, tool calls silent.
    Text,
    /// One JSON result object emitted at end (no live output).
    Json,
    /// JSONL events: one JSON object per line on stdout as they happen (for TUIs, log analyzers).
    StreamJson,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum InputFormat {
    /// Prompt comes from -p as a plain string (default).
    Text,
    /// Reserved: prompt comes as JSONL events on stdin. Currently treated as text.
    StreamJson,
}

static SEED_SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/seed-skills");
static SANDBOX_PROFILE: &str = include_str!("../sandbox.sb");

const MAX_DEPTH: u32 = 5;
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 120;
const DEFAULT_TOOL_OUTPUT_MAX_BYTES: usize = 200_000;
const TOOL_READ_CHUNK_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug)]
struct ToolLimits {
    timeout: Duration,
    output_max_bytes: usize,
}

#[derive(Debug)]
struct CapturedOutput {
    bytes: Vec<u8>,
    total_bytes: usize,
}

impl CapturedOutput {
    fn was_truncated(&self) -> bool {
        self.total_bytes > self.bytes.len()
    }
}

impl ToolLimits {
    fn from_env() -> Self {
        let timeout_secs = env::var("LAZAR_TOOL_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|secs: &u64| *secs > 0)
            .unwrap_or(DEFAULT_TOOL_TIMEOUT_SECS);
        let output_max_bytes = env::var("LAZAR_TOOL_OUTPUT_MAX_BYTES")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|bytes: &usize| *bytes > 0)
            .unwrap_or(DEFAULT_TOOL_OUTPUT_MAX_BYTES);

        Self {
            timeout: Duration::from_secs(timeout_secs),
            output_max_bytes,
        }
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn safe_prefix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }

    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn truncate_tool_output(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        return output.to_string();
    }

    let prefix = safe_prefix(output, max_bytes);
    let omitted = output.len().saturating_sub(prefix.len());
    format!("{prefix}\n[truncated: {omitted} bytes omitted]")
}

fn tool_input_preview(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }

    let prefix = safe_prefix(input, max_bytes);
    let omitted = input.len().saturating_sub(prefix.len());
    format!("{prefix}…[+{omitted}b]")
}

fn validate_reset_home(home: &Path) -> Result<(), String> {
    if home.as_os_str().is_empty() {
        return Err("refusing to reset an empty LAZAR_HOME".into());
    }
    if home.is_relative() {
        return Err(format!(
            "refusing to reset relative LAZAR_HOME: {}",
            home.display()
        ));
    }
    if home.parent().is_none() {
        return Err(format!(
            "refusing to reset filesystem root: {}",
            home.display()
        ));
    }
    if let Ok(user_home) = env::var("HOME") {
        if Path::new(&user_home) == home {
            return Err(format!(
                "refusing to reset HOME directly: {}",
                home.display()
            ));
        }
    }

    Ok(())
}

fn emit_stream_error(format: OutputFormat, message: &str) {
    if format == OutputFormat::StreamJson {
        emit_event(json!({"type": "error", "message": message}));
    }
}

fn env_flag_enabled(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

#[derive(Parser)]
#[command(name = "lazar", about = "The smallest self-evolving agent.")]
struct Args {
    /// Prompt for the agent.
    #[arg(short, long, conflicts_with = "reset_all")]
    prompt: Option<String>,

    /// Wipe skills/, memory/, workspace/, logs/ and re-seed from the kernel.
    #[arg(long)]
    reset_all: bool,

    /// Skip confirmation for --reset-all.
    #[arg(long)]
    yes: bool,

    /// Verbose mode: prints tool calls, depth, and stop_reason to stderr.
    #[arg(long)]
    verbose: bool,

    /// Override the model. Falls back to $LAZAR_MODEL or claude-sonnet-4-6.
    #[arg(long)]
    model: Option<String>,

    /// Output format: 'text' (default, streams human-readable), 'json' (single result object at end), or 'stream-json' (JSONL events as they happen).
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    output_format: OutputFormat,

    /// Input format: 'text' (default; prompt from -p) or 'stream-json' (reserved; treated as text).
    #[arg(long, value_enum, default_value_t = InputFormat::Text)]
    input_format: InputFormat,
}

fn lazar_home() -> PathBuf {
    // Allow override via LAZAR_HOME for non-default install locations.
    if let Ok(p) = env::var("LAZAR_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    PathBuf::from(env::var("HOME").expect("HOME unset")).join("lazar")
}

fn reset_all(skip_confirm: bool) -> Result<(), Box<dyn std::error::Error>> {
    let home = lazar_home();
    validate_reset_home(&home)
        .map_err(|msg| std::io::Error::new(std::io::ErrorKind::InvalidInput, msg))?;

    if !skip_confirm {
        eprint!(
            "Wipe skills/, memory/, workspace/, logs/ in {} and reseed?\n[y/N] ",
            home.display()
        );
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim(), "y" | "Y" | "yes") {
            eprintln!("aborted.");
            return Ok(());
        }
    }

    for sub in ["skills", "memory", "workspace", "logs"] {
        let p = home.join(sub);
        if p.exists() {
            fs::remove_dir_all(&p)?;
        }
        fs::create_dir_all(&p)?;
    }

    SEED_SKILLS.extract(home.join("skills"))?;
    eprintln!(
        "[lazar] reset complete. {} seed files written.",
        SEED_SKILLS.files().count()
    );
    Ok(())
}

fn read_capped<R>(mut reader: R, cap: usize) -> thread::JoinHandle<CapturedOutput>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut kept = Vec::with_capacity(cap.min(TOOL_READ_CHUNK_BYTES));
        let mut total = 0usize;
        let mut buf = [0u8; TOOL_READ_CHUNK_BYTES];

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    total = total.saturating_add(n);
                    if kept.len() < cap {
                        let remaining = cap - kept.len();
                        kept.extend_from_slice(&buf[..n.min(remaining)]);
                    }
                }
                Err(_) => break,
            }
        }

        CapturedOutput {
            bytes: kept,
            total_bytes: total,
        }
    })
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}

fn run_bash(cmd: &str) -> String {
    let limits = ToolLimits::from_env();
    let lazar_path = lazar_home();
    let lazar = lazar_path.to_string_lossy().to_string();
    let workspace = format!("{lazar}/workspace");

    if let Err(e) = fs::create_dir_all(&workspace) {
        return format!("[workspace error: {e}]\n[exit 1]");
    }

    let current_depth = env::var("LAZAR_DEPTH")
        .ok()
        .and_then(|d| d.parse::<u32>().ok())
        .unwrap_or(0);
    let child_depth = current_depth.saturating_add(1);

    let mut command = Command::new("/usr/bin/sandbox-exec");
    command
        .arg("-D")
        .arg(format!("SKILLS_PATH={lazar}/skills"))
        .arg("-D")
        .arg(format!("MEMORY_PATH={lazar}/memory"))
        .arg("-D")
        .arg(format!("WORKSPACE_PATH={lazar}/workspace"))
        .arg("-D")
        .arg(format!("LOGS_PATH={lazar}/logs"))
        .arg("-p")
        .arg(SANDBOX_PROFILE)
        .arg("/bin/bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .env(
            "PATH",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        )
        .env("HOME", &lazar)
        .env("LAZAR_HOME", &lazar)
        .env("LAZAR_SKILLS", format!("{lazar}/skills"))
        .env("LAZAR_MEMORY", format!("{lazar}/memory"))
        .env("LAZAR_WORKSPACE", format!("{lazar}/workspace"))
        .env("LAZAR_LOGS", format!("{lazar}/logs"))
        .env("LAZAR_TOOL_ENV", "1")
        .env("LAZAR_DEPTH", child_depth.to_string())
        .env(
            "TERM",
            env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
        )
        .env(
            "LANG",
            env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()),
        );

    if let Ok(model) = env::var("LAZAR_MODEL") {
        command.env("LAZAR_MODEL", model);
    }

    if env_flag_enabled("LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY") {
        command.env("LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY", "1");
        if let Ok(api_key) = env::var("ANTHROPIC_API_KEY") {
            command.env("ANTHROPIC_API_KEY", api_key);
        }
    }

    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return format!("[spawn error: {e}]\n[exit 1]"),
    };

    let stdout = child
        .stdout
        .take()
        .map(|out| read_capped(out, limits.output_max_bytes));
    let stderr = child
        .stderr
        .take()
        .map(|err| read_capped(err, limits.output_max_bytes));

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() >= limits.timeout {
                    timed_out = true;
                    kill_process_group(child.id());
                    let _ = child.kill();
                    break child.wait().ok();
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                return format!("[wait error: {e}]\n[exit 1]");
            }
        }
    };

    let stdout = stdout
        .and_then(|h| h.join().ok())
        .unwrap_or(CapturedOutput {
            bytes: vec![],
            total_bytes: 0,
        });
    let stderr = stderr
        .and_then(|h| h.join().ok())
        .unwrap_or(CapturedOutput {
            bytes: vec![],
            total_bytes: 0,
        });

    let mut result = String::from_utf8_lossy(&stdout.bytes).to_string();
    if stdout.was_truncated() {
        result.push_str(&format!(
            "\n[stdout truncated: {} bytes omitted]",
            stdout.total_bytes.saturating_sub(stdout.bytes.len())
        ));
    }

    if !stderr.bytes.is_empty() || stderr.was_truncated() {
        let err = String::from_utf8_lossy(&stderr.bytes);
        result.push_str("\n[stderr]\n");
        result.push_str(&err);
        if stderr.was_truncated() {
            result.push_str(&format!(
                "\n[stderr truncated: {} bytes omitted]",
                stderr.total_bytes.saturating_sub(stderr.bytes.len())
            ));
        }
    }

    if timed_out {
        result.push_str(&format!("\n[timeout after {}s]", limits.timeout.as_secs()));
    }
    result.push_str(&format!(
        "\n[exit {}]",
        status.and_then(|s| s.code()).unwrap_or(-1)
    ));

    truncate_tool_output(&result, limits.output_max_bytes)
}

/// Emit a single JSON event to stdout (used when --format=json). Adds ts_ms.
/// This is the "structured streaming" output for programmatic consumers like
/// the TUI. Distinct from the canonical log at logs/stream.jsonl.
fn emit_event(mut event: Value) {
    use std::io::Write;
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Some(obj) = event.as_object_mut() {
        obj.insert("ts_ms".into(), json!(ts_ms));
    }
    println!("{event}");
    let _ = std::io::stdout().flush();
}

fn unique_log_archive_path(logs: &Path) -> PathBuf {
    let pid = std::process::id();
    for attempt in 0..1000u32 {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!(".{attempt}")
        };
        let candidate = logs.join(format!("stream.jsonl.{}.{pid}{suffix}.bak", now_nanos()));
        if !candidate.exists() {
            return candidate;
        }
    }

    logs.join(format!("stream.jsonl.{}.{pid}.fallback.bak", now_nanos()))
}

#[cfg(unix)]
fn lock_file_exclusive(file: &File) {
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_EX);
    }
}

#[cfg(not(unix))]
fn lock_file_exclusive(_file: &File) {}

fn ensure_dir_not_symlink(path: &Path, label: &str) -> std::io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to use symlinked {label}"),
            ));
        }
    }
    fs::create_dir_all(path)?;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing to use symlinked {label}"),
        ));
    }
    Ok(())
}

fn open_append_no_follow(path: &Path, label: &str) -> std::io::Result<File> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to append to symlinked {label}"),
            ));
        }
    }

    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path)
}

fn write_new_no_follow(path: &Path, label: &str, contents: &str) -> std::io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to write symlinked {label}"),
            ));
        }
    }

    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = opts.open(path)?;
    file.write_all(contents.as_bytes())
}

/// Auto-rotate the stream log when it exceeds LAZAR_LOG_MAX_BYTES (default 10MB).
/// The current log is moved to a unique stream.jsonl.<unix-nanos>.<pid>.bak
/// archive and a minimal summary is written into memory/log-summaries/ so the
/// agent has a navigable index of past archives. Skills like _meta/log-rotation
/// can layer richer summaries on top, but this floor is non-negotiable —
/// without it the log grows unbounded and any context-loading attempt
/// blows the API limit.
fn maybe_rotate_log(logs: &Path, path: &Path) {
    let max_bytes: u64 = env::var("LAZAR_LOG_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_485_760); // 10 MB

    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return, // file doesn't exist yet — nothing to rotate
    };
    if meta.file_type().is_symlink() {
        eprintln!("[lazar] WARN: refusing to rotate symlinked stream.jsonl");
        return;
    }

    let size = meta.len();
    if size < max_bytes {
        return;
    }

    let archive = unique_log_archive_path(logs);
    if fs::rename(path, &archive).is_err() {
        eprintln!("[lazar] WARN: log rotation rename failed; log will keep growing");
        return;
    }
    eprintln!(
        "[lazar] auto-rotated stream.jsonl → {} ({size} bytes)",
        archive
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("archive")
    );

    // Write a minimal summary into memory/log-summaries/ so the agent has
    // an index of past archives without re-reading them. _meta/log-rotation
    // and _meta/distill can layer richer per-archive summaries on top.
    let memory_root = logs
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("memory");
    let memory = memory_root.join("log-summaries");
    if let Err(e) = ensure_dir_not_symlink(&memory_root, "memory directory") {
        eprintln!("[lazar] WARN: not writing rotation summary: {e}");
        return;
    }
    if let Err(e) = ensure_dir_not_symlink(&memory, "log-summaries directory") {
        eprintln!("[lazar] WARN: not writing rotation summary: {e}");
        return;
    }
    let summary_name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("stream.jsonl.unknown.bak")
        .trim_start_matches("stream.jsonl.")
        .trim_end_matches(".bak")
        .to_string();
    let sum_path = memory.join(format!("{summary_name}.md"));
    let header = format!(
        "# log-summary {summary_name}\n\
         \n\
         archive: {}\n\
         size_bytes: {size}\n\
         rotated_at_unix_ms: {}\n\
         \n\
         _Auto-rotated by the lazar kernel when stream.jsonl exceeded \
         LAZAR_LOG_MAX_BYTES. For richer per-archive summaries (top user prompts, \
         top tool commands), run the `_meta/log-rotation` skill against this archive. \
         For LLM-extracted learnings, run `_meta/distill`._\n",
        archive.display(),
        now_millis()
    );
    if let Err(e) = write_new_no_follow(&sum_path, "rotation summary", &header) {
        eprintln!("[lazar] WARN: failed to write rotation summary: {e}");
    }
}

/// Append a single JSON event to the canonical stream at logs/stream.jsonl.
/// The kernel records; the agent decides what (if anything) to read back.
/// This is the "infinite memory log" — a single append-only stream of every
/// prompt, response, tool call, and result across all invocations.
///
/// Auto-rotates when the log exceeds LAZAR_LOG_MAX_BYTES.
fn append_stream(mut event: Value) {
    let logs = lazar_home().join("logs");
    let _ = fs::create_dir_all(&logs);
    let path = logs.join("stream.jsonl");
    let lock_path = logs.join(".stream.lock");

    let lock = open_append_no_follow(&lock_path, "stream lock").ok();
    if let Some(lock) = lock.as_ref() {
        lock_file_exclusive(lock);
    }

    // Safety floor: rotate before appending if oversized. Without this,
    // the agent's own load-context calls eventually blow the API limit.
    maybe_rotate_log(&logs, &path);

    if let Some(obj) = event.as_object_mut() {
        obj.insert("ts_ms".into(), json!(now_millis()));
    }

    match open_append_no_follow(&path, "stream log") {
        Ok(mut f) => {
            let _ = writeln!(f, "{}", event);
        }
        Err(e) => eprintln!("[lazar] WARN: failed to append stream log: {e}"),
    }
}

/// Parse an SSE stream from the Anthropic Messages API.
///
/// In `Text` mode: streams text deltas to stdout as they arrive (live).
/// In `StreamJson` mode: emits structured JSONL events to stdout. Tool inputs
/// stream as `input_json_delta` chunks but are only emitted as a complete
/// `tool_use` event on `content_block_stop`, once the partial JSON has been
/// fully accumulated and parsed.
///
/// Either way, the function reassembles the full assistant content array and
/// returns it together with the final stop_reason for the agent loop.
fn parse_sse_stream(
    resp: reqwest::blocking::Response,
    format: OutputFormat,
) -> Result<(Value, String), Box<dyn std::error::Error>> {
    parse_sse_reader(std::io::BufReader::new(resp), format)
}

fn parse_sse_reader<R: BufRead>(
    reader: R,
    format: OutputFormat,
) -> Result<(Value, String), Box<dyn std::error::Error>> {
    use std::collections::HashMap;

    let mut blocks: HashMap<u64, Value> = HashMap::new();
    let mut tool_input_buffers: HashMap<u64, String> = HashMap::new();
    let mut stop_reason = String::new();
    let mut printed_any_text = false;
    let mut saw_message_stop = false;

    for line in reader.lines() {
        let line = line?;
        let data = match line.strip_prefix("data: ") {
            Some(d) => d,
            None => continue,
        };
        if data.is_empty() {
            continue;
        }

        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        match v["type"].as_str().unwrap_or("") {
            "content_block_start" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                blocks.insert(idx, v["content_block"].clone());
            }
            "content_block_delta" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                let delta = &v["delta"];
                match delta["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or("");
                        match format {
                            OutputFormat::Text => {
                                print!("{text}");
                                let _ = std::io::stdout().flush();
                                printed_any_text = true;
                            }
                            OutputFormat::StreamJson => {
                                emit_event(json!({
                                    "type": "text_delta",
                                    "index": idx,
                                    "text": text,
                                }));
                            }
                            OutputFormat::Json => {
                                // Suppress live output; final result emitted at end_turn.
                            }
                        }
                        if let Some(block) = blocks.get_mut(&idx) {
                            let current = block["text"].as_str().unwrap_or("").to_string();
                            block["text"] = json!(current + text);
                        }
                    }
                    "input_json_delta" => {
                        let partial = delta["partial_json"].as_str().unwrap_or("");
                        tool_input_buffers.entry(idx).or_default().push_str(partial);
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                if let Some(block) = blocks.get_mut(&idx) {
                    if block["type"] == "tool_use" {
                        let buf = tool_input_buffers.remove(&idx).unwrap_or_default();
                        // Diagnostic: surface parse failures and empty buffers
                        // unconditionally to stderr — these are the root cause of
                        // empty-command tool calls and you want to see them even
                        // without --verbose.
                        let input: Value = match serde_json::from_str(&buf) {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!(
                                    "[lazar] WARN: tool_use input parse failed (err={e}); buffer ({} bytes) was: {:?}",
                                    buf.len(),
                                    tool_input_preview(&buf, 500)
                                );
                                json!({})
                            }
                        };
                        if buf.is_empty() {
                            eprintln!(
                                "[lazar] WARN: tool_use idx={idx} had no input_json_delta events; input is {{}}"
                            );
                        }
                        block["input"] = input;
                        if format == OutputFormat::StreamJson {
                            emit_event(json!({
                                "type": "tool_use",
                                "index": idx,
                                "id": block["id"],
                                "name": block["name"],
                                "input": block["input"],
                            }));
                        }
                    } else if block["type"] == "text" && format == OutputFormat::StreamJson {
                        emit_event(json!({
                            "type": "text_done",
                            "index": idx,
                        }));
                    }
                }
            }
            "message_delta" => {
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    stop_reason = r.to_string();
                }
            }
            "message_stop" => {
                saw_message_stop = true;
                break;
            }
            "error" => {
                let msg = v["error"]["message"].as_str().unwrap_or("unknown");
                if format == OutputFormat::StreamJson {
                    emit_event(json!({"type": "error", "message": msg}));
                }
                return Err(format!("stream error: {msg}").into());
            }
            _ => {}
        }
    }

    if !saw_message_stop {
        return Err("SSE stream ended before message_stop".into());
    }

    if format == OutputFormat::Text && printed_any_text {
        println!();
    }

    let mut indices: Vec<u64> = blocks.keys().copied().collect();
    indices.sort();
    let content: Vec<Value> = indices
        .iter()
        .filter_map(|i| blocks.get(i).cloned())
        .collect();

    Ok((json!(content), stop_reason))
}

fn run_agent(
    prompt: &str,
    format: OutputFormat,
    verbose: bool,
    model_override: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let api_key = match env::var("ANTHROPIC_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            let msg = "ANTHROPIC_API_KEY is not set";
            emit_stream_error(format, msg);
            return Err(msg.into());
        }
    };
    let model = model_override
        .or_else(|| env::var("LAZAR_MODEL").ok())
        .unwrap_or_else(|| DEFAULT_MODEL.into());
    let depth: u32 = env::var("LAZAR_DEPTH")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0);

    if depth > MAX_DEPTH {
        let msg = format!("recursion depth {depth} exceeds max {MAX_DEPTH}");
        emit_stream_error(format, &msg);
        return Err(msg.into());
    }

    let home = lazar_home();
    let skills = home.join("skills");

    let system = format!(
        "You are lazar, a self-evolving agent.\n\
         \n\
         You have ONE tool: execute(command). It runs a bash command \
         through sandbox-exec and returns stdout+stderr+exit code.\n\
         \n\
         ARCHITECTURE\n\
         Your home is {home_disp}. Inside it:\n\
         - bin/   and src/   are READ-ONLY (the immutable kernel). You may \
         `cat` them to study yourself; writes are blocked by the OS.\n\
         - skills/  your \"being\" — capabilities you can read and grow.\n\
         - memory/  durable notes (managed via the memory skill).\n\
         - workspace/  scratchpad and your cwd (write freely).\n\
         - logs/stream.jsonl  the canonical event stream (see CONTEXT).\n\
         \n\
         You evolve by writing skills, never by modifying your runner. If \
         you need a capability you lack, write a new SKILL.md under \
         {skills_disp}/<name>/ and append a one-line entry to {skills_disp}/INDEX.md.\n\
         \n\
         CONTEXT (this is important)\n\
         Each `lazar -p` invocation starts with NO prior messages. Your \
         conversational context is just this prompt and the system prompt.\n\
         \n\
         BUT every prompt, response, tool call, and tool result you (and \
         past-you) ever produced is appended as JSONL to:\n\
             {home_disp}/logs/stream.jsonl\n\
         \n\
         This is your \"infinite memory\" log. The kernel records but does \
         NOT auto-load — reading and hygiene are skill territory.\n\
         \n\
         CRITICAL HYGIENE\n\
         - BEFORE reading the log for the first time in a session, read \
         _meta/load-context/SKILL.md and use its bounded recipes. NEVER \
         `cat` the log — it can be MB+ and will exceed your context window \
         (the API will reject the request).\n\
         - The log auto-rotates at LAZAR_LOG_MAX_BYTES (default ~10MB). \
         Run _meta/log-rotation only when you want richer archive summaries. \
         A cheap size check at session start is good hygiene: \
         `wc -c {home_disp}/logs/stream.jsonl`\n\
         \n\
         If a prompt is referential (\"yes\", \"do that\", \"continue\", \"as I \
         said\"), use load-context. The default recipe is:\n\
             tail -n 200 {home_disp}/logs/stream.jsonl | tail -c 50000\n\
         The log format is one JSON object per line with fields like \
         {{kind, ts_ms, content, command, ...}}; jq is your friend.\n\
         \n\
         SELF-DISCOVERY\n\
         You can read anywhere in the filesystem, including your own \
         kernel source at {home_disp}/src/ and the binary at {home_disp}/bin/. \
         Study yourself when useful. If you perceive a kernel-level \
         limitation, you cannot change the kernel — but you can almost \
         always work around it by writing a skill. Reach for that move \
         before assuming something is impossible.\n\
         \n\
         FIRST MOVE\n\
         For any non-trivial task, start with: cat {skills_disp}/INDEX.md\n\
         Then read the relevant SKILL.md before acting. Skills carry the \
         hard-won detail you need.\n\
         \n\
         RECURSION\n\
         Tool subprocesses do not inherit ANTHROPIC_API_KEY by default. \
         If the operator set LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY=1, \
         nested calls can use: execute `LAZAR_DEPTH={next_depth} lazar -p \"...\"`. \
         Otherwise treat nested `lazar -p` API calls as unavailable and ask \
         the user to re-run with that opt-in if recursion is essential.\n\
         Recursion depth is capped at {max_depth}; current depth is {depth}.\n\
         \n\
         BOUNDARIES\n\
         Writes are limited by sandbox-exec to skills/, memory/, \
         workspace/, logs/, and /tmp. Do not try to modify bin/ or src/, \
         dotfiles, ssh keys, or anything outside ~/lazar. You will \
         see 'Operation not permitted' if you try; learn from the failure \
         and route around it via a skill.",
        home_disp = home.display(),
        skills_disp = skills.display(),
        next_depth = depth + 1,
        max_depth = MAX_DEPTH,
        depth = depth
    );

    let tool = json!({
        "name": "execute",
        "description": "Run a bash command. Returns stdout+stderr+exit code. \
            Recurse to yourself via `lazar -p`. Read skills via `cat`. \
            Self-modify by writing files under skills/.",
        "input_schema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash command to execute"
                }
            },
            "required": ["command"]
        }
    });

    let mut messages: Vec<Value> = vec![json!({"role": "user", "content": prompt})];
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            let msg = format!("reqwest client build failed: {e}");
            emit_stream_error(format, &msg);
            return Err(msg.into());
        }
    };

    append_stream(json!({"kind": "invoke_start", "depth": depth, "model": model}));
    append_stream(json!({"kind": "user", "content": prompt}));

    if format == OutputFormat::StreamJson {
        emit_event(json!({
            "type": "invoke_start",
            "depth": depth,
            "model": &model,
            "prompt": prompt,
        }));
    }

    if verbose {
        eprintln!("[lazar] invoke_start depth={depth} model={model}");
    }

    // Tight-loop safeguard: if the model emits N consecutive turns where every
    // tool_use has an empty/missing command, abort. Without this the kernel
    // happily runs `bash -c ""` forever.
    let mut consecutive_empty_turns: u32 = 0;
    const MAX_CONSECUTIVE_EMPTY_TURNS: u32 = 3;

    loop {
        let body = json!({
            "model": model,
            "max_tokens": 8192,
            "system": system,
            "tools": [tool],
            "messages": messages,
            "stream": true,
        });

        let resp = match client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
        {
            Ok(resp) => resp,
            Err(e) => {
                let msg = format!("API request failed: {e}");
                emit_stream_error(format, &msg);
                append_stream(json!({"kind": "invoke_end", "stop_reason": "error", "note": &msg}));
                return Err(msg.into());
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            let msg = format!("API {status}: {text}");
            // Mirror the in-stream "error" event so stream-json consumers
            // (e.g. the TUI) see a structured message instead of just stderr
            // + non-zero exit.
            emit_stream_error(format, &msg);
            if verbose {
                eprintln!("[lazar] {msg}");
            }
            append_stream(json!({"kind": "invoke_end", "stop_reason": "error", "note": &msg}));
            return Err(msg.into());
        }

        let (content, stop_reason_str) = match parse_sse_stream(resp, format) {
            Ok(parsed) => parsed,
            Err(e) => {
                let msg = e.to_string();
                emit_stream_error(format, &msg);
                append_stream(json!({"kind": "invoke_end", "stop_reason": "error", "note": &msg}));
                return Err(msg.into());
            }
        };
        messages.push(json!({"role": "assistant", "content": content.clone()}));
        append_stream(json!({"kind": "assistant", "content": content.clone()}));

        let stop_reason = stop_reason_str.as_str();

        if stop_reason == "end_turn" {
            // text was already streamed live in parse_sse_stream (Text mode)
            append_stream(json!({"kind": "invoke_end", "stop_reason": stop_reason}));
            let duration_ms = started.elapsed().as_millis();

            if format == OutputFormat::StreamJson {
                emit_event(json!({
                    "type": "invoke_end",
                    "stop_reason": stop_reason,
                    "duration_ms": duration_ms,
                }));
            } else if format == OutputFormat::Json {
                let final_text: String = content
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|b| {
                                if b["type"] == "text" {
                                    b["text"].as_str().map(String::from)
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                let result = json!({
                    "type": "result",
                    "stop_reason": stop_reason,
                    "model": &model,
                    "result": final_text,
                    "content": content,
                    "duration_ms": duration_ms,
                });
                println!("{result}");
            }

            if verbose {
                eprintln!("[lazar] invoke_end stop_reason={stop_reason} duration={duration_ms}ms");
            }
            return Ok(());
        }

        let mut results = vec![];
        let mut had_any_valid_call = false;
        for b in content.as_array().unwrap_or(&vec![]) {
            if b["type"] == "tool_use" && b["name"] == "execute" {
                let cmd_raw = b["input"]["command"].as_str().unwrap_or("");
                let cmd = cmd_raw.trim();

                let output = if cmd.is_empty() {
                    // Refuse empty commands. Surface a clear error to the model
                    // instead of running `bash -c ""` and getting [exit 0]. This
                    // prevents the kernel from being a silent accomplice in tight
                    // loops of malformed tool calls.
                    if verbose {
                        eprintln!("[lazar] tool_use: <EMPTY COMMAND — refused>");
                    }
                    "[error: 'execute' was called with an empty or missing 'command' field. \
                     Provide a non-empty bash command, or end your turn instead of calling \
                     the tool with no arguments.]\n[exit 1]"
                        .to_string()
                } else {
                    had_any_valid_call = true;
                    if verbose {
                        let preview: String = cmd.chars().take(120).collect();
                        eprintln!("[lazar] tool_use: {preview}");
                    }
                    run_bash(cmd)
                };

                let result_event = json!({
                    "type": "tool_result",
                    "tool_use_id": b["id"],
                    "content": output,
                });
                append_stream(json!({
                    "kind": "tool_result",
                    "tool_use_id": b["id"],
                    "command": cmd_raw,
                    "content": result_event["content"],
                }));
                if format == OutputFormat::StreamJson {
                    emit_event(json!({
                        "type": "tool_result",
                        "tool_use_id": b["id"],
                        "command": cmd_raw,
                        "content": output,
                    }));
                }
                if verbose {
                    eprintln!("[lazar] tool_result: {} bytes", output.len());
                }
                results.push(result_event);
            }
        }

        // Empty-turn detection: if this turn had tool_use blocks but none had
        // a valid command, we made no real progress. Track and abort after
        // MAX_CONSECUTIVE_EMPTY_TURNS in a row.
        if !results.is_empty() && !had_any_valid_call {
            consecutive_empty_turns += 1;
            if consecutive_empty_turns >= MAX_CONSECUTIVE_EMPTY_TURNS {
                let msg = format!(
                    "aborted after {consecutive_empty_turns} consecutive turns with only empty tool calls"
                );
                eprintln!("[lazar] {msg}");
                append_stream(
                    json!({"kind": "invoke_end", "stop_reason": "aborted", "note": &msg}),
                );
                if format == OutputFormat::StreamJson {
                    emit_event(json!({"type": "error", "message": &msg}));
                }
                return Err(msg.into());
            }
        } else if had_any_valid_call {
            consecutive_empty_turns = 0;
        }

        if results.is_empty() {
            // text already streamed in parse_sse_stream; just log and exit
            eprintln!("[lazar] stop_reason={stop_reason}, no tool calls; exiting");
            append_stream(
                json!({"kind": "invoke_end", "stop_reason": stop_reason, "note": "no tool calls"}),
            );
            if format == OutputFormat::StreamJson {
                emit_event(
                    json!({"type": "invoke_end", "stop_reason": stop_reason, "note": "no tool calls"}),
                );
            } else if format == OutputFormat::Json {
                println!(
                    "{}",
                    json!({
                        "type": "result",
                        "stop_reason": stop_reason,
                        "model": &model,
                        "result": "",
                        "content": content,
                        "note": "no tool calls",
                    })
                );
            }
            return Ok(());
        }

        messages.push(json!({"role": "user", "content": results}));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if args.reset_all {
        return reset_all(args.yes);
    }

    let prompt = args
        .prompt
        .ok_or("must pass -p <prompt> or --reset-all (see --help)")?;

    // input_format is reserved for now; both values currently consume the prompt as text.
    let _ = args.input_format;

    run_agent(&prompt, args.output_format, args.verbose, args.model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::Path;

    #[test]
    fn env_flag_accepts_only_explicit_truthy_values() {
        let name = format!("LAZAR_TEST_FLAG_{}_{}", std::process::id(), now_millis());

        env::remove_var(&name);
        assert!(!env_flag_enabled(&name));

        env::set_var(&name, "yes");
        assert!(env_flag_enabled(&name));

        env::set_var(&name, "0");
        assert!(!env_flag_enabled(&name));
        env::remove_var(&name);
    }

    #[cfg(unix)]
    #[test]
    fn append_and_write_helpers_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = env::temp_dir().join(format!(
            "lazar-symlink-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target");
        let append_link = dir.join("append-link");
        let write_link = dir.join("write-link");
        fs::write(&target, "existing").unwrap();
        symlink(&target, &append_link).unwrap();
        symlink(&target, &write_link).unwrap();

        let append_err = open_append_no_follow(&append_link, "test append").unwrap_err();
        let write_err = write_new_no_follow(&write_link, "test write", "new").unwrap_err();

        assert!(append_err.to_string().contains("symlinked"));
        assert!(write_err.to_string().contains("symlinked"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncates_tool_results_with_omitted_byte_count() {
        let original = "a".repeat(80);
        let truncated = truncate_tool_output(&original, 25);

        assert!(truncated.len() <= 80);
        assert!(truncated.contains("truncated"));
        assert!(truncated.contains("55 bytes omitted"));
    }

    #[test]
    fn tool_input_preview_handles_multibyte_boundaries() {
        let input = "é".repeat(400);
        let preview = tool_input_preview(&input, 501);

        assert!(preview.contains("…[+"));
        assert!(preview.is_char_boundary(preview.len()));
    }

    #[test]
    fn sse_parser_rejects_eof_before_message_stop() {
        let data = b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n";
        let err = parse_sse_reader(Cursor::new(&data[..]), OutputFormat::Json).unwrap_err();

        assert!(err.to_string().contains("ended before message_stop"));
    }

    #[test]
    fn unique_archive_path_does_not_reuse_existing_archive() {
        let dir = env::temp_dir().join(format!(
            "lazar-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();

        let first = unique_log_archive_path(&dir);
        fs::write(&first, "existing").unwrap();
        let second = unique_log_archive_path(&dir);

        assert_ne!(first, second);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reset_home_validation_rejects_user_home() {
        let home = env::var("HOME").unwrap();
        let err = validate_reset_home(Path::new(&home)).unwrap_err();

        assert!(err.contains("refusing to reset HOME"));
    }
}
