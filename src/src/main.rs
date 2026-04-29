//! lazar — the smallest self-evolving agent harness.
//!
//! One tool: `execute(command)` runs bash through sandbox-exec.
//! Everything else lives as skills under ~/lazar/skills/.
//! Seed skills are embedded in the binary so `--reset-all` is a
//! true factory restore.

use clap::{Parser, ValueEnum};
use include_dir::{include_dir, Dir};
use serde_json::{json, Value};
use std::{env, fs, path::PathBuf, process::Command};

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

fn run_bash(cmd: &str) -> String {
    let lazar_path = lazar_home();
    let lazar = lazar_path.to_string_lossy().to_string();
    let workspace = format!("{lazar}/workspace");

    // make sure workspace exists; sandbox-exec needs a real cwd
    let _ = fs::create_dir_all(&workspace);

    let result = Command::new("sandbox-exec")
        .arg("-D").arg(format!("SKILLS_PATH={lazar}/skills"))
        .arg("-D").arg(format!("MEMORY_PATH={lazar}/memory"))
        .arg("-D").arg(format!("WORKSPACE_PATH={lazar}/workspace"))
        .arg("-D").arg(format!("LOGS_PATH={lazar}/logs"))
        .arg("-p").arg(SANDBOX_PROFILE)
        .arg("bash").arg("-c").arg(cmd)
        .current_dir(&workspace)
        // Export env vars so skills can use $LAZAR_HOME / $LAZAR_SKILLS / etc.
        // instead of hardcoded paths. This is what makes skills portable to
        // any agent that exports LAZAR_HOME.
        .env("LAZAR_HOME", &lazar)
        .env("LAZAR_SKILLS", format!("{lazar}/skills"))
        .env("LAZAR_MEMORY", format!("{lazar}/memory"))
        .env("LAZAR_WORKSPACE", format!("{lazar}/workspace"))
        .env("LAZAR_LOGS", format!("{lazar}/logs"))
        .output();

    match result {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.is_empty() {
                s.push_str(&format!("\n[stderr]\n{err}"));
            }
            s.push_str(&format!("\n[exit {}]", o.status.code().unwrap_or(-1)));
            s
        }
        Err(e) => format!("[spawn error: {e}]"),
    }
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

/// Auto-rotate the stream log when it exceeds LAZAR_LOG_MAX_BYTES (default 10MB).
/// The current log is moved to stream.jsonl.<unix_secs>.bak and a minimal
/// summary is written into memory/log-summaries/<unix_secs>.md so the agent
/// has a navigable index of past archives. Skills like _meta/log-rotation
/// can layer richer summaries on top, but this floor is non-negotiable —
/// without it the log grows unbounded and any context-loading attempt
/// blows the API limit.
fn maybe_rotate_log(logs: &PathBuf, path: &PathBuf) {
    let max_bytes: u64 = env::var("LAZAR_LOG_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_485_760); // 10 MB

    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return, // file doesn't exist yet — nothing to rotate
    };
    let size = meta.len();
    if size < max_bytes {
        return;
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let archive = logs.join(format!("stream.jsonl.{ts}.bak"));
    if fs::rename(path, &archive).is_err() {
        eprintln!("[lazar] WARN: log rotation rename failed; log will keep growing");
        return;
    }
    eprintln!(
        "[lazar] auto-rotated stream.jsonl → stream.jsonl.{ts}.bak ({size} bytes)"
    );

    // Write a minimal summary into memory/log-summaries/ so the agent has
    // an index of past archives without re-reading them. _meta/log-rotation
    // and _meta/distill can layer richer per-archive summaries on top.
    let memory = lazar_home().join("memory/log-summaries");
    let _ = fs::create_dir_all(&memory);
    let sum_path = memory.join(format!("{ts}.md"));
    let header = format!(
        "# log-summary {ts}\n\
         \n\
         archive: {}\n\
         size_bytes: {size}\n\
         rotated_at_unix: {ts}\n\
         \n\
         _Auto-rotated by the lazar kernel when stream.jsonl exceeded \
         LAZAR_LOG_MAX_BYTES. For richer per-archive summaries (top user prompts, \
         top tool commands), run the `_meta/log-rotation` skill against this archive. \
         For LLM-extracted learnings, run `_meta/distill`._\n",
        archive.display()
    );
    let _ = fs::write(sum_path, header);
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

    // Safety floor: rotate before appending if oversized. Without this,
    // the agent's own load-context calls eventually blow the API limit.
    maybe_rotate_log(&logs, &path);

    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    if let Some(obj) = event.as_object_mut() {
        obj.insert("ts_ms".into(), json!(ts_ms));
    }

    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{}", event);
    }
}

/// Parse an SSE stream from the Anthropic Messages API.
///
/// In `Text` mode: streams text deltas to stdout as they arrive (live).
/// In `Json` mode: emits structured JSONL events to stdout (text_delta,
/// text_done, tool_use). Tool inputs stream as `input_json_delta` chunks but
/// are only emitted as a complete `tool_use` event on `content_block_stop`,
/// once the partial JSON has been fully accumulated and parsed.
///
/// Either way, the function reassembles the full assistant content array and
/// returns it together with the final stop_reason for the agent loop.
fn parse_sse_stream(
    resp: reqwest::blocking::Response,
    format: OutputFormat,
) -> Result<(Value, String), Box<dyn std::error::Error>> {
    use std::collections::HashMap;
    use std::io::{BufRead, Write};

    let mut blocks: HashMap<u64, Value> = HashMap::new();
    let mut tool_input_buffers: HashMap<u64, String> = HashMap::new();
    let mut stop_reason = String::new();
    let mut printed_any_text = false;

    let reader = std::io::BufReader::new(resp);

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
                        tool_input_buffers
                            .entry(idx)
                            .or_default()
                            .push_str(partial);
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
                                    if buf.len() > 500 { format!("{}…[+{}b]", &buf[..500], buf.len() - 500) } else { buf.clone() }
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
            "message_stop" => break,
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
    let started = std::time::Instant::now();
    let api_key = env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY is not set")?;
    let model = model_override
        .or_else(|| env::var("LAZAR_MODEL").ok())
        .unwrap_or_else(|| DEFAULT_MODEL.into());
    let depth: u32 = env::var("LAZAR_DEPTH")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0);

    if depth > MAX_DEPTH {
        eprintln!("[lazar] recursion depth {depth} exceeds max {MAX_DEPTH}");
        std::process::exit(2);
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
         - The log grows without bound. Run _meta/log-rotation when it \
         exceeds ~10MB. A cheap size check at session start is good hygiene: \
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
         To delegate a sub-task: execute `LAZAR_DEPTH={next_depth} lazar -p \"...\"`.\n\
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
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("reqwest client build failed: {e}"))?;

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

        let resp = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            let msg = format!("API {status}: {text}");
            // Mirror the in-stream "error" event so stream-json consumers
            // (e.g. the TUI) see a structured message instead of just stderr
            // + non-zero exit.
            if format == OutputFormat::StreamJson {
                emit_event(json!({"type": "error", "message": msg}));
            }
            if verbose {
                eprintln!("[lazar] {msg}");
            }
            return Err(msg.into());
        }

        let (content, stop_reason_str) = parse_sse_stream(resp, format)?;
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
                append_stream(json!({"kind": "invoke_end", "stop_reason": "aborted", "note": &msg}));
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
            append_stream(json!({"kind": "invoke_end", "stop_reason": stop_reason, "note": "no tool calls"}));
            if format == OutputFormat::StreamJson {
                emit_event(json!({"type": "invoke_end", "stop_reason": stop_reason, "note": "no tool calls"}));
            } else if format == OutputFormat::Json {
                println!("{}", json!({
                    "type": "result",
                    "stop_reason": stop_reason,
                    "model": &model,
                    "result": "",
                    "content": content,
                    "note": "no tool calls",
                }));
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
