//! lazar hooks — drop-in lifecycle hook system.
//!
//! Hooks are bash scripts under `$LAZAR_HOME/hooks/<event>.d/` that fire at
//! deterministic moments in the agent lifecycle. They receive a JSON payload
//! on stdin and may emit a JSON action on stdout to influence behavior.
//!
//! Discovery is per-process: the first call to `fire` for a given event
//! caches the directory listing for the rest of the invocation.
//!
//! Hooks run through the same sandbox profile as the agent's own bash, so
//! they can read everywhere but only write to skills/, memory/, workspace/,
//! logs/, and /tmp. The kernel itself is immutable to them.

use serde_json::{json, Value};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use crate::{append_stream, lazar_home, now_millis, SANDBOX_PROFILE};

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 5;
const HOOK_STDOUT_CAP_BYTES: usize = 16_384;
const HOOK_STDERR_CAP_BYTES: usize = 8_192;

/// Lifecycle events. `as_str` is the directory name under `hooks/`
/// (suffixed `.d`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookEvent {
    SessionStart,
    UserPrompt,
    PreTool,
    PostTool,
    SessionEnd,
    LogRotation,
    AgentStop,
    Tick,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::SessionStart => "session-start",
            HookEvent::UserPrompt => "user-prompt",
            HookEvent::PreTool => "pre-tool",
            HookEvent::PostTool => "post-tool",
            HookEvent::SessionEnd => "session-end",
            HookEvent::LogRotation => "log-rotation",
            HookEvent::AgentStop => "agent-stop",
            HookEvent::Tick => "tick",
        }
    }
}

/// What a hook is asking the kernel to do.
#[derive(Clone, Debug)]
pub enum HookAction {
    /// Default. Hook ran, no behavior change requested.
    Continue,
    /// Pre-tool only: block this tool call. Reason is surfaced as the
    /// tool result so the model can recover.
    Veto { reason: String },
    /// Pre-tool only: rewrite the bash command before exec.
    TransformCommand(String),
    /// Post-tool only: rewrite the captured tool output before it
    /// gets posted back to the model.
    TransformOutput(String),
    /// Session-start / user-prompt: append context to the model's system
    /// prompt or user prompt. Currently advisory — wired in on a per-event
    /// basis by the caller.
    InjectContext(String),
}

#[derive(Clone, Debug)]
pub struct HookResult {
    pub script: PathBuf,
    pub action: HookAction,
    pub exit_code: i32,
    pub duration_ms: u128,
    pub timed_out: bool,
}

impl HookResult {
    pub fn veto_reason(&self) -> Option<&str> {
        match &self.action {
            HookAction::Veto { reason } => Some(reason.as_str()),
            _ => None,
        }
    }
    pub fn transform_command(&self) -> Option<&str> {
        match &self.action {
            HookAction::TransformCommand(cmd) => Some(cmd.as_str()),
            _ => None,
        }
    }
    pub fn transform_output(&self) -> Option<&str> {
        match &self.action {
            HookAction::TransformOutput(out) => Some(out.as_str()),
            _ => None,
        }
    }
    pub fn inject_context(&self) -> Option<&str> {
        match &self.action {
            HookAction::InjectContext(ctx) => Some(ctx.as_str()),
            _ => None,
        }
    }
}

/// Per-process discovery cache. Avoids re-scanning hook dirs on every fire.
static DISCOVERY_CACHE: Mutex<Option<DiscoveryCache>> = Mutex::new(None);

struct DiscoveryCache {
    home: PathBuf,
    by_event: std::collections::HashMap<&'static str, Vec<PathBuf>>,
}

fn discovery_for(event: HookEvent) -> Vec<PathBuf> {
    let home = lazar_home();
    let mut guard = DISCOVERY_CACHE.lock().expect("hook discovery cache poisoned");
    let cache = guard.get_or_insert_with(|| DiscoveryCache {
        home: home.clone(),
        by_event: Default::default(),
    });
    if cache.home != home {
        // LAZAR_HOME changed mid-process (rare, but possible in tests).
        // Rebuild from scratch.
        cache.home = home.clone();
        cache.by_event.clear();
    }
    cache
        .by_event
        .entry(event.as_str())
        .or_insert_with(|| scan_hooks_dir(&home, event))
        .clone()
}

fn scan_hooks_dir(home: &Path, event: HookEvent) -> Vec<PathBuf> {
    let dir = home.join("hooks").join(format!("{}.d", event.as_str()));
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return vec![], // no dir = no hooks; not an error
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            if !p.is_file() {
                return false;
            }
            let name = match p.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => return false,
            };
            // Skip .disabled examples and dot-files.
            if name.ends_with(".disabled") || name.starts_with('.') {
                return false;
            }
            // Must be executable. On non-unix, accept anything.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = p.metadata() {
                    return meta.permissions().mode() & 0o111 != 0;
                }
                false
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
        .collect();
    paths.sort();
    paths
}

/// Fire all scripts in `hooks/<event>.d/`. Returns one HookResult per script.
/// Errors don't propagate — a misbehaving hook is logged and treated as Continue.
pub fn fire(event: HookEvent, payload: Value) -> Vec<HookResult> {
    let scripts = discovery_for(event);
    if scripts.is_empty() {
        return vec![];
    }

    let timeout_secs: u64 = env::var("LAZAR_HOOK_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|s: &u64| *s > 0)
        .unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS);
    let timeout = Duration::from_secs(timeout_secs);

    let mut payload = payload;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("event".into(), json!(event.as_str()));
        obj.insert("ts_ms".into(), json!(now_millis()));
        obj.insert("lazar_home".into(), json!(lazar_home()));
    }
    let payload_str = payload.to_string();

    let mut results = Vec::with_capacity(scripts.len());
    for script in scripts {
        let result = run_hook(&script, &payload_str, timeout, event);
        results.push(result);
    }
    results
}

fn run_hook(
    script: &Path,
    payload_str: &str,
    timeout: Duration,
    event: HookEvent,
) -> HookResult {
    let started = Instant::now();
    let script_name = script
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string();

    append_stream(json!({
        "kind": "hook_start",
        "event": event.as_str(),
        "script": script_name.clone(),
    }));

    let lazar = lazar_home().to_string_lossy().to_string();
    let workspace = format!("{lazar}/workspace");
    let _ = fs::create_dir_all(&workspace);

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
        .arg(script)
        .current_dir(&workspace)
        .stdin(Stdio::piped())
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
        .env("LAZAR_HOOK_EVENT", event.as_str())
        .env("LAZAR_HOOK_PAYLOAD_LEN", payload_str.len().to_string())
        .env(
            "TERM",
            env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
        )
        .env(
            "LANG",
            env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()),
        );

    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("hook spawn failed: {e}");
            eprintln!("[lazar] WARN: {msg} ({})", script.display());
            append_stream(json!({
                "kind": "hook_end",
                "event": event.as_str(),
                "script": script_name,
                "action": "continue",
                "exit_code": -1,
                "duration_ms": started.elapsed().as_millis(),
                "note": msg,
            }));
            return HookResult {
                script: script.to_path_buf(),
                action: HookAction::Continue,
                exit_code: -1,
                duration_ms: started.elapsed().as_millis(),
                timed_out: false,
            };
        }
    };

    // Start output readers before writing stdin. A hook may ignore stdin;
    // writing a large payload synchronously would otherwise block before the
    // timeout loop ever starts.
    let stdout_handle = child
        .stdout
        .take()
        .map(|out| crate::read_capped(out, HOOK_STDOUT_CAP_BYTES));
    let stderr_handle = child
        .stderr
        .take()
        .map(|err| crate::read_capped(err, HOOK_STDERR_CAP_BYTES));

    let stdin_handle = child.stdin.take().map(|mut stdin| {
        let payload = payload_str.as_bytes().to_vec();
        thread::spawn(move || {
            let _ = stdin.write_all(&payload);
        })
    });

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    timed_out = true;
                    crate::kill_process_group(child.id());
                    let _ = child.kill();
                    break child.wait().ok();
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                break None;
            }
        }
    };

    if let Some(handle) = stdin_handle {
        let _ = handle.join();
    }

    let stdout_bytes = stdout_handle
        .and_then(|h| h.join().ok())
        .map(|c| c.bytes)
        .unwrap_or_default();
    let stderr_bytes = stderr_handle
        .and_then(|h| h.join().ok())
        .map(|c| c.bytes)
        .unwrap_or_default();

    let exit_code = status.and_then(|s| s.code()).unwrap_or(-1);
    let duration_ms = started.elapsed().as_millis();

    if !stderr_bytes.is_empty() {
        let stderr_str = String::from_utf8_lossy(&stderr_bytes);
        eprintln!(
            "[lazar] hook {} stderr ({}):\n{}",
            script_name,
            event.as_str(),
            stderr_str.trim_end()
        );
    }

    let action = parse_action(&stdout_bytes, event, &script_name);

    append_stream(json!({
        "kind": "hook_end",
        "event": event.as_str(),
        "script": script_name,
        "action": action_label(&action),
        "exit_code": exit_code,
        "duration_ms": duration_ms,
        "timed_out": timed_out,
    }));

    HookResult {
        script: script.to_path_buf(),
        action,
        exit_code,
        duration_ms,
        timed_out,
    }
}

fn parse_action(stdout_bytes: &[u8], event: HookEvent, script_name: &str) -> HookAction {
    // Empty stdout = continue (the common case).
    let trimmed_start = stdout_bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(stdout_bytes.len());
    if trimmed_start == stdout_bytes.len() {
        return HookAction::Continue;
    }

    let s = match std::str::from_utf8(stdout_bytes) {
        Ok(s) => s.trim(),
        Err(_) => {
            eprintln!(
                "[lazar] WARN: hook {} ({}) emitted non-UTF8 stdout; treating as Continue",
                script_name,
                event.as_str()
            );
            return HookAction::Continue;
        }
    };

    let value: Value = match serde_json::from_str(s) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[lazar] WARN: hook {} ({}) emitted invalid JSON ({e}); treating as Continue",
                script_name,
                event.as_str()
            );
            return HookAction::Continue;
        }
    };

    let action = value.get("action").and_then(|v| v.as_str()).unwrap_or("continue");
    match action {
        "continue" => HookAction::Continue,
        "veto" => {
            if !matches!(event, HookEvent::PreTool) {
                eprintln!(
                    "[lazar] WARN: hook {} returned 'veto' on event {}; veto is pre-tool only — ignoring",
                    script_name,
                    event.as_str()
                );
                return HookAction::Continue;
            }
            let reason = value
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("vetoed by hook")
                .to_string();
            HookAction::Veto { reason }
        }
        "transform" => {
            if let Some(cmd) = value.get("command").and_then(|v| v.as_str()) {
                if !matches!(event, HookEvent::PreTool) {
                    eprintln!(
                        "[lazar] WARN: hook {} returned 'transform.command' on event {}; pre-tool only — ignoring",
                        script_name,
                        event.as_str()
                    );
                    return HookAction::Continue;
                }
                return HookAction::TransformCommand(cmd.to_string());
            }
            if let Some(out) = value.get("output").and_then(|v| v.as_str()) {
                if !matches!(event, HookEvent::PostTool) {
                    eprintln!(
                        "[lazar] WARN: hook {} returned 'transform.output' on event {}; post-tool only — ignoring",
                        script_name,
                        event.as_str()
                    );
                    return HookAction::Continue;
                }
                return HookAction::TransformOutput(out.to_string());
            }
            eprintln!(
                "[lazar] WARN: hook {} returned 'transform' without command/output; treating as Continue",
                script_name
            );
            HookAction::Continue
        }
        "inject" => {
            if !matches!(event, HookEvent::SessionStart | HookEvent::UserPrompt) {
                eprintln!(
                    "[lazar] WARN: hook {} returned 'inject' on event {}; only session-start/user-prompt — ignoring",
                    script_name,
                    event.as_str()
                );
                return HookAction::Continue;
            }
            let ctx = value
                .get("context")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            HookAction::InjectContext(ctx)
        }
        other => {
            eprintln!(
                "[lazar] WARN: hook {} returned unknown action '{other}'; treating as Continue",
                script_name
            );
            HookAction::Continue
        }
    }
}

fn action_label(a: &HookAction) -> &'static str {
    match a {
        HookAction::Continue => "continue",
        HookAction::Veto { .. } => "veto",
        HookAction::TransformCommand(_) => "transform_command",
        HookAction::TransformOutput(_) => "transform_output",
        HookAction::InjectContext(_) => "inject",
    }
}

/// Helper: collect first veto from a result vec, if any.
pub fn first_veto(results: &[HookResult]) -> Option<&str> {
    results.iter().find_map(|r| r.veto_reason())
}

/// Helper: collect last command transform (last-wins).
pub fn last_transform_command(results: &[HookResult]) -> Option<&str> {
    results.iter().rev().find_map(|r| r.transform_command())
}

/// Helper: collect last output transform (last-wins).
pub fn last_transform_output(results: &[HookResult]) -> Option<&str> {
    results.iter().rev().find_map(|r| r.transform_output())
}

/// Helper: collect all injected contexts in order, joined with newlines.
pub fn join_injected_contexts(results: &[HookResult]) -> String {
    let mut out = String::new();
    for r in results {
        if let Some(ctx) = r.inject_context() {
            if !ctx.is_empty() {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(ctx);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_action_continue_on_empty() {
        let action = parse_action(b"", HookEvent::PreTool, "test");
        assert!(matches!(action, HookAction::Continue));
    }

    #[test]
    fn parse_action_continue_on_whitespace_only() {
        let action = parse_action(b"   \n\t  ", HookEvent::PreTool, "test");
        assert!(matches!(action, HookAction::Continue));
    }

    #[test]
    fn parse_action_veto_pre_tool() {
        let action = parse_action(
            br#"{"action":"veto","reason":"denied"}"#,
            HookEvent::PreTool,
            "test",
        );
        match action {
            HookAction::Veto { reason } => assert_eq!(reason, "denied"),
            _ => panic!("expected Veto"),
        }
    }

    #[test]
    fn parse_action_veto_ignored_on_post_tool() {
        let action = parse_action(
            br#"{"action":"veto","reason":"too late"}"#,
            HookEvent::PostTool,
            "test",
        );
        assert!(matches!(action, HookAction::Continue));
    }

    #[test]
    fn parse_action_transform_command() {
        let action = parse_action(
            br#"{"action":"transform","command":"ls -la"}"#,
            HookEvent::PreTool,
            "test",
        );
        match action {
            HookAction::TransformCommand(c) => assert_eq!(c, "ls -la"),
            _ => panic!("expected TransformCommand"),
        }
    }

    #[test]
    fn parse_action_transform_output() {
        let action = parse_action(
            br#"{"action":"transform","output":"redacted"}"#,
            HookEvent::PostTool,
            "test",
        );
        match action {
            HookAction::TransformOutput(o) => assert_eq!(o, "redacted"),
            _ => panic!("expected TransformOutput"),
        }
    }

    #[test]
    fn parse_action_invalid_json_falls_back_to_continue() {
        let action = parse_action(b"not json", HookEvent::PreTool, "test");
        assert!(matches!(action, HookAction::Continue));
    }

    #[test]
    fn parse_action_unknown_action_falls_back_to_continue() {
        let action = parse_action(
            br#"{"action":"explode"}"#,
            HookEvent::PreTool,
            "test",
        );
        assert!(matches!(action, HookAction::Continue));
    }

    #[test]
    fn helpers_pick_correct_results() {
        let results = vec![
            HookResult {
                script: PathBuf::from("a"),
                action: HookAction::Continue,
                exit_code: 0,
                duration_ms: 1,
                timed_out: false,
            },
            HookResult {
                script: PathBuf::from("b"),
                action: HookAction::TransformCommand("first".into()),
                exit_code: 0,
                duration_ms: 1,
                timed_out: false,
            },
            HookResult {
                script: PathBuf::from("c"),
                action: HookAction::TransformCommand("last".into()),
                exit_code: 0,
                duration_ms: 1,
                timed_out: false,
            },
        ];
        assert_eq!(last_transform_command(&results), Some("last"));
        assert_eq!(first_veto(&results), None);
    }

    #[test]
    fn join_injected_contexts_skips_empty_and_separates() {
        let results = vec![
            HookResult {
                script: PathBuf::from("a"),
                action: HookAction::InjectContext("first".into()),
                exit_code: 0,
                duration_ms: 1,
                timed_out: false,
            },
            HookResult {
                script: PathBuf::from("b"),
                action: HookAction::InjectContext("".into()),
                exit_code: 0,
                duration_ms: 1,
                timed_out: false,
            },
            HookResult {
                script: PathBuf::from("c"),
                action: HookAction::InjectContext("third".into()),
                exit_code: 0,
                duration_ms: 1,
                timed_out: false,
            },
        ];
        assert_eq!(join_injected_contexts(&results), "first\n\nthird");
    }
}
