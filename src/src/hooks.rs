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
//! /tmp, and specific session/auth subdirectories under ~/.codex and ~/.claude
//! (not their full config trees). logs/ is trusted kernel-owned state: hooks may
//! read it via `$LAZAR_LOGS`, but archive mutation belongs to the kernel.

use serde_json::{json, Value};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use crate::log::append_stream;
use crate::{lazar_home, now_millis};

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 5;
const HOOK_STDOUT_CAP_BYTES: usize = 16_384;
const HOOK_STDERR_CAP_BYTES: usize = 8_192;
const MAX_HOOKS_PER_EVENT: usize = 32;

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
    /// Session-start / user-prompt: appended to the system prompt under
    /// "RUNTIME CONTEXT (from hooks)" (see `collect_hook_context`).
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
    let mut guard = DISCOVERY_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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

fn scan_hooks_dir(_home: &Path, event: HookEvent) -> Vec<PathBuf> {
    let dir = crate::user_dir("hooks").join(format!("{}.d", event.as_str()));
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
    if paths.len() > MAX_HOOKS_PER_EVENT {
        eprintln!(
            "[lazar] WARN: {} has {} hooks; running the first {}",
            event.as_str(),
            paths.len(),
            MAX_HOOKS_PER_EVENT
        );
        paths.truncate(MAX_HOOKS_PER_EVENT);
    }
    paths
}

/// Fire all scripts in `hooks/<event>.d/`. Returns one HookResult per script.
/// Hook failures normally continue, but strict pre-tool hooks fail closed.
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
        if crate::core::invocation::is_cancelled() {
            break;
        }
        let result = run_hook(&script, &payload_str, timeout, event);
        results.push(result);
    }
    results
}

fn hook_failure_action(event: HookEvent, script_name: &str, msg: String) -> HookAction {
    if matches!(event, HookEvent::PreTool) {
        HookAction::Veto {
            reason: format!("pre-tool hook {script_name} failed: {msg}"),
        }
    } else {
        HookAction::Continue
    }
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

    if crate::env_flag_enabled("LAZAR_TRACE_HOOKS") {
        crate::emit_event(json!({"type": "hook_start", "event": event.as_str(), "script": &script_name}));
    }
    append_stream(json!({
        "kind": "hook_start",
        "event": event.as_str(),
        "script": script_name.clone(),
    }));

    let lazar = lazar_home().to_string_lossy().to_string();
    let workspace = format!("{lazar}/workspace");
    let _ = fs::create_dir_all(&workspace);

    // Hooks run in the same trust mode as the agent: the runtime access
    // policy decides both the sandbox wrapper and the HOME the script sees,
    // exactly as run_bash does. (main/acp set LAZAR_NO_SANDBOX and the
    // policy together, so this is the single source of truth.)
    let policy = crate::access::current();
    let unsandboxed = policy.unsandboxed();
    let mut command = crate::spawn::bash_command(
        crate::spawn::BashPayload::Script(script),
        &lazar,
        &policy,
    );

    let bash_home = crate::spawn::bash_home(&lazar, unsandboxed);

    command
        .current_dir(&workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::spawn::apply_base_env(&mut command, &lazar, &bash_home, &policy);
    command
        .env("LAZAR_HOOK_EVENT", event.as_str())
        .env("LAZAR_HOOK_PAYLOAD_LEN", payload_str.len().to_string());
    if let Some(session_id) = crate::core::invocation::session_id() {
        command.env("LAZAR_SESSION_ID", session_id);
    }

    let (mut child, _owner) = match crate::spawn::spawn_owned(command) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("hook spawn failed: {e}");
            eprintln!("[lazar] WARN: {msg} ({})", script.display());
            let action = hook_failure_action(event, &script_name, msg.clone());
            append_stream(json!({
                "kind": "hook_end",
                "event": event.as_str(),
                "script": script_name,
                "action": action_label(&action),
                "exit_code": -1,
                "duration_ms": started.elapsed().as_millis(),
                "note": msg,
            }));
            return HookResult {
                script: script.to_path_buf(),
                action,
                exit_code: -1,
                duration_ms: started.elapsed().as_millis(),
                timed_out: false,
            };
        }
    };
    let child_pid = child.id();

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

    let (status, timed_out) = crate::spawn::wait_with_timeout(
        &mut child,
        child_pid,
        timeout,
        Duration::from_millis(20),
    )
    .unwrap_or_default();

    // A hook may background a process that inherits its captured pipes. The
    // hook shell can then exit while the reader threads remain blocked forever.
    // Hooks own their process group, so terminate descendants before joining.
    crate::kill_process_group(child_pid);

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

    let mut action = parse_action(&stdout_bytes, event, &script_name);

    if matches!(event, HookEvent::PreTool) && !matches!(action, HookAction::Veto { .. }) {
        if timed_out {
            action = hook_failure_action(event, &script_name, "timed out".to_string());
        } else if exit_code != 0 {
            action = hook_failure_action(
                event,
                &script_name,
                format!("exited with status {exit_code}"),
            );
        }
    }

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

/// Reject invalid hook output: warn the operator, then fail open
/// (Continue) or closed (Veto) depending on strict mode. `lenient_note`
/// describes the fail-open consequence ("ignoring", "treating as Continue").
fn reject(event: HookEvent, script_name: &str, msg: String, lenient_note: &str) -> HookAction {
    eprintln!(
        "[lazar] WARN: hook {} ({}) {}; {}",
        script_name,
        event.as_str(),
        msg,
        if matches!(event, HookEvent::PreTool) { "failing closed" } else { lenient_note }
    );
    hook_failure_action(event, script_name, msg)
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
            return reject(event, script_name, "emitted non-UTF8 stdout".to_string(), "treating as Continue");
        }
    };

    let value: Value = match serde_json::from_str(s) {
        Ok(v) => v,
        Err(e) => {
            return reject(event, script_name, format!("emitted invalid JSON ({e})"), "treating as Continue");
        }
    };

    let Some(action) = value.get("action").and_then(|v| v.as_str()) else {
        return reject(event, script_name, "emitted JSON without string action".to_string(), "treating as Continue");
    };
    match action {
        "continue" => HookAction::Continue,
        "veto" => {
            if !matches!(event, HookEvent::PreTool) {
                let msg = format!("returned 'veto' on event {}; veto is pre-tool only", event.as_str());
                return reject(event, script_name, msg, "ignoring");
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
                    let msg = format!("returned 'transform.command' on event {}; pre-tool only", event.as_str());
                    return reject(event, script_name, msg, "ignoring");
                }
                return HookAction::TransformCommand(cmd.to_string());
            }
            if let Some(out) = value.get("output").and_then(|v| v.as_str()) {
                if !matches!(event, HookEvent::PostTool) {
                    let msg = format!("returned 'transform.output' on event {}; post-tool only", event.as_str());
                    return reject(event, script_name, msg, "ignoring");
                }
                return HookAction::TransformOutput(out.to_string());
            }
            reject(event, script_name, "returned 'transform' without command/output".to_string(), "treating as Continue")
        }
        "inject" => {
            if !matches!(event, HookEvent::SessionStart | HookEvent::UserPrompt) {
                let msg = format!(
                    "returned 'inject' on event {}; only session-start/user-prompt",
                    event.as_str()
                );
                return reject(event, script_name, msg, "ignoring");
            }
            let Some(ctx) = value.get("context").and_then(|v| v.as_str()) else {
                return reject(event, script_name, "returned 'inject' without string context".to_string(), "using empty context");
            };
            HookAction::InjectContext(ctx.to_string())
        }
        other => reject(event, script_name, format!("returned unknown action '{other}'"), "treating as Continue"),
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
                let separator = if out.is_empty() { "" } else { "\n\n" };
                if out.len().saturating_add(separator.len())
                    >= crate::core::context::MAX_HOOK_CONTEXT_BYTES
                {
                    break;
                }
                out.push_str(separator);
                let remaining =
                    crate::core::context::MAX_HOOK_CONTEXT_BYTES.saturating_sub(out.len());
                let bounded = crate::text::head_bytes(ctx, remaining);
                out.push_str(bounded);
                if bounded.len() < ctx.len() {
                    break;
                }
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
    fn injected_hook_context_has_one_aggregate_budget() {
        let results = (0..3)
            .map(|n| HookResult {
                script: PathBuf::from(format!("{n}.sh")),
                action: HookAction::InjectContext("x".repeat(crate::core::context::MAX_HOOK_CONTEXT_BYTES / 2)),
                exit_code: 0,
                duration_ms: 1,
                timed_out: false,
            })
            .collect::<Vec<_>>();
        let joined = join_injected_contexts(&results);
        assert_eq!(joined.len(), crate::core::context::MAX_HOOK_CONTEXT_BYTES);
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
    fn parse_action_invalid_json_vetoes_pre_tool_and_continues_post_tool() {
        let pre = parse_action(b"not json", HookEvent::PreTool, "test");
        assert_veto_contains(pre, "invalid JSON");
        let post = parse_action(b"not json", HookEvent::PostTool, "test");
        assert!(matches!(post, HookAction::Continue));
    }

    #[test]
    fn parse_action_unknown_action_vetoes_every_pre_tool_hook() {
        let non_strict = parse_action(
            br#"{"action":"explode"}"#,
            HookEvent::PreTool,
            "guard",
        );
        assert_veto_contains(non_strict, "unknown action");

        let strict = parse_action(
            br#"{"action":"explode"}"#,
            HookEvent::PreTool,
            "guard.strict",
        );
        assert_veto_contains(strict, "unknown action");
    }

    #[test]
    fn strict_transform_without_payload_vetoes() {
        let action = parse_action(
            br#"{"action":"transform"}"#,
            HookEvent::PreTool,
            "guard.strict",
        );

        assert_veto_contains(action, "transform");
    }

    #[test]
    fn strict_hook_that_prints_transform_then_fails_vetoes() {
        let env = HookTestEnv::new("strict-transform-fails");
        let script = env.write_script(
            "rewrite.strict",
            r#"printf '%s\n' '{"action":"transform","command":"echo rewritten"}'; exit 7"#,
        );

        let result = run_hook(&script, "{}", Duration::from_secs(1), HookEvent::PreTool);

        assert_eq!(result.exit_code, 7);
        assert!(!result.timed_out);
        assert_veto_contains(result.action, "exited with status 7");
    }

    #[test]
    fn strict_hook_that_prints_transform_then_times_out_vetoes() {
        let env = HookTestEnv::new("strict-transform-times-out");
        let script = env.write_script(
            "rewrite.strict",
            r#"printf '%s\n' '{"action":"transform","command":"echo rewritten"}'; sleep 1"#,
        );

        let result = run_hook(&script, "{}", Duration::from_millis(50), HookEvent::PreTool);

        assert!(result.timed_out);
        assert_veto_contains(result.action, "timed out");
    }

    fn assert_veto_contains(action: HookAction, expected: &str) {
        match action {
            HookAction::Veto { reason } => assert!(
                reason.contains(expected),
                "expected veto reason to contain {expected:?}, got {reason:?}"
            ),
            other => panic!("expected Veto containing {expected:?}, got {other:?}"),
        }
    }

    struct HookTestEnv {
        root: PathBuf,
        old_lazar_home: Option<std::ffi::OsString>,
        old_no_sandbox: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl HookTestEnv {
        fn new(label: &str) -> Self {
            // Mutates LAZAR_HOME: serialize with every other env-mutating test.
            let guard = crate::test_support::env_lock();
            let root = std::env::temp_dir().join(format!(
                "lazar-hook-{label}-{}-{}",
                std::process::id(),
                crate::now_millis()
            ));
            std::fs::create_dir_all(root.join("logs")).unwrap();
            std::fs::create_dir_all(root.join("workspace")).unwrap();

            let old_lazar_home = std::env::var_os("LAZAR_HOME");
            let old_no_sandbox = std::env::var_os("LAZAR_NO_SANDBOX");
            std::env::set_var("LAZAR_HOME", &root);
            std::env::set_var("LAZAR_NO_SANDBOX", "1");

            Self {
                root,
                old_lazar_home,
                old_no_sandbox,
                _guard: guard,
            }
        }

        fn write_script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.root.join(name);
            std::fs::write(&path, format!("#!/bin/bash\nset -euo pipefail\n{body}\n")).unwrap();
            path
        }
    }

    impl Drop for HookTestEnv {
        fn drop(&mut self) {
            match &self.old_lazar_home {
                Some(value) => std::env::set_var("LAZAR_HOME", value),
                None => std::env::remove_var("LAZAR_HOME"),
            }
            match &self.old_no_sandbox {
                Some(value) => std::env::set_var("LAZAR_NO_SANDBOX", value),
                None => std::env::remove_var("LAZAR_NO_SANDBOX"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
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
