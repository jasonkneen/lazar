//! lazar — the smallest self-evolving agent harness.
//!
//! One tool: `bash(command)` runs bash through sandbox-exec.
//! Everything else lives as skills under ~/lazar/skills/.
//! Seed skills are embedded in the binary so `--reset-all` is a
//! true factory restore.

use clap::{Parser, Subcommand, ValueEnum};
use futures::channel::mpsc::UnboundedSender;
use include_dir::{include_dir, Dir};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use std::io::BufRead;

mod access;
mod acp;
mod core;
mod hooks;
mod log;
mod providers;
mod session;
mod jobs;
mod skill;
mod spawn;
mod text;
use hooks::HookEvent;
use log::append_stream;

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    /// Human-readable: streams assistant text to stdout, tool calls silent.
    Text,
    /// One JSON result object emitted at end (no live output).
    Json,
    /// JSONL events: one JSON object per line on stdout as they happen (for TUIs, log analyzers).
    StreamJson,
}

static SEED_SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/seed-skills");
static SEED_HOOKS: Dir = include_dir!("$CARGO_MANIFEST_DIR/seed-hooks");
pub(crate) static SANDBOX_PROFILE: &str = include_str!("../sandbox.sb");
static SYSTEM_PROMPT_TEMPLATE: &str = include_str!("../system-prompt.txt");

const MAX_DEPTH: u32 = 5;
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 120;
const DEFAULT_TOOL_OUTPUT_MAX_BYTES: usize = core::context::MAX_TOOL_OUTPUT_BYTES;
const DEFAULT_MAX_AGENT_TURNS: u32 = 50;
const DEFAULT_MAX_TOOL_CALLS: u32 = 200;
const DEFAULT_MAX_TOKENS: u32 = core::context::MAX_OUTPUT_TOKENS;
const TOOL_READ_CHUNK_BYTES: usize = 8192;
const MAX_SSE_LINE_BYTES: usize = 512 * 1024;
const INHERIT_PROVIDER_KEY_ENV: &str = "LAZAR_TOOL_INHERIT_PROVIDER_KEY";
const LEGACY_INHERIT_PROVIDER_KEY_ENV: &str = "LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY";
const GUARDRAILS_MODE_FILE: &str = ".guardrails";

#[derive(Clone, Copy, Debug)]
struct ToolLimits {
    timeout: Duration,
    output_max_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct CapturedOutput {
    pub(crate) bytes: Vec<u8>,
    pub(crate) total_bytes: usize,
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
            .unwrap_or(DEFAULT_TOOL_OUTPUT_MAX_BYTES)
            .min(core::context::MAX_CONTEXT_ITEM_BYTES);

        Self {
            timeout: Duration::from_secs(timeout_secs),
            output_max_bytes,
        }
    }
}

pub(crate) fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn truncate_tool_output(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        return output.to_string();
    }

    let prefix = text::head_bytes(output, max_bytes);
    let omitted = output.len().saturating_sub(prefix.len());
    format!("{prefix}\n[truncated: {omitted} bytes omitted; re-run a narrower command such as head, tail, or sed -n to read the rest]")
}

fn tool_input_preview(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }

    let prefix = text::head_bytes(input, max_bytes);
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
    if home
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "refusing to reset non-canonical LAZAR_HOME: {}",
            home.display()
        ));
    }
    if let Ok(meta) = fs::symlink_metadata(home) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "refusing to reset symlinked LAZAR_HOME: {}",
                home.display()
            ));
        }
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

/// Read the operator-owned persisted guardrails mode. The file sits at the
/// Lazar root, outside every sandbox write allowlist. Invalid or indirect
/// state fails closed instead of silently widening access.
fn persisted_guardrails_off(home: &Path) -> Result<bool, String> {
    let user_path = home.join("user").join(GUARDRAILS_MODE_FILE);
    let legacy = home.join(GUARDRAILS_MODE_FILE);
    let path = if user_path.exists() { user_path } else { legacy };
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(format!("cannot inspect {}: {err}", path.display())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "guardrails mode must be a regular file, not a symlink or directory: {}",
            path.display()
        ));
    }
    if metadata.len() > 16 {
        return Err(format!("guardrails mode file is oversized: {}", path.display()));
    }
    let mode = fs::read_to_string(&path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    match mode.trim() {
        "on" => Ok(false),
        "off" => Ok(true),
        other => Err(format!(
            "invalid guardrails mode {other:?} in {}; expected 'on' or 'off'",
            path.display()
        )),
    }
}

fn emit_stream_error(format: OutputFormat, message: &str) {
    if format == OutputFormat::StreamJson {
        emit_event(json!({"type": "error", "message": message}));
    }
}

pub(crate) fn env_flag_enabled(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

fn is_terminal_stop_reason(stop_reason: &str) -> bool {
    matches!(stop_reason, "end_turn" | "stop_sequence" | "refusal")
}

fn runtime_identity_context(
    model: &str,
    api_url: &str,
    provider: Option<&str>,
    client: Option<&acp::AcpClient>,
) -> String {
    let endpoint = api_url
        .split_once("://")
        .map(|(scheme, rest)| {
            let authority = rest.split('/').next().unwrap_or(rest);
            let host = authority.rsplit('@').next().unwrap_or(authority);
            format!("{scheme}://{host}")
        })
        .unwrap_or_else(|| "custom Anthropic-compatible endpoint".into());
    let provider_line = provider
        .map(|id| format!("Provider: {id} (from providers.toml)\n"))
        .unwrap_or_default();
    let client_line = match client {
        Some(c) => format!("Client: {}\n", c.describe()),
        None => "Client: direct CLI invocation (lazar -p); no ACP client attached\n".to_string(),
    };
    format!(
        "AUTHORITATIVE INVOCATION IDENTITY\n\
         Active model: {model}\n\
         API endpoint: {endpoint}\n\
         {provider_line}\
         {client_line}\
         These values come from the current kernel invocation and override any stale model, provider, or client identity in hook-injected context, memory, or prior session text. Never claim to be a different model or provider. The client line tells you who is rendering your output and what it can do on its own side; the only tool YOU have is still bash."
    )
}

#[derive(Parser)]
#[command(name = "lazar", about = "The smallest self-evolving agent.")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// `structured`: end every answer with a typed record (reply, speech,
    /// sources, verification) for rich clients. Default `raw`.
    #[arg(long, value_name = "raw|structured", value_parser = ["raw", "structured"])]
    answer_format: Option<String>,

    /// Prompt for the agent.
    #[arg(short, long, conflicts_with = "reset_all")]
    prompt: Option<String>,

    /// Wipe skills/, hooks/, memory/, workspace/, logs/, and saved access
    /// approvals, then re-seed from the kernel.
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

    /// Heartbeat: fire all hooks under hooks/tick.d/ and exit. No model call.
    /// Wire this to launchd / cron for scheduled work (memory consolidation,
    /// summary enrichment, etc).
    #[arg(long, conflicts_with_all = ["prompt", "reset_all"])]
    tick: bool,

    /// Session id for multi-turn conversation continuity. When set, prior
    /// turns from logs/sessions/<id>.jsonl are prepended to the message
    /// array, and new events are appended to that log. Same flag both
    /// creates the log on first turn and continues it on subsequent turns.
    /// Allowed chars: a-z A-Z 0-9 - _ . (max 64 chars, no path traversal).
    #[arg(long, conflicts_with = "resume")]
    session: Option<String>,

    /// Resume the most recently modified session log automatically.
    /// Equivalent to `--session <newest>`. Useful for referential
    /// prompts ("ok do that", "yes", "fine") when you don't remember
    /// the session id. The agent can also use this on a nested
    /// `execute lazar --resume -p "..."` call to disambiguate a
    /// referential prompt against full prior context.
    #[arg(long)]
    resume: bool,

    /// Run bash under sandbox-exec. Off by default.
    /// Also enabled via LAZAR_SANDBOX=1 or user/.guardrails containing `on`.
    #[arg(long)]
    sandbox: bool,

    /// Force sandbox off for this invocation. This is the default.
    #[arg(long, conflicts_with = "sandbox")]
    no_sandbox: bool,

    /// Set the working directory used by every model-issued bash command.
    /// The path must already exist and is canonicalized before any hook or
    /// tool runs.
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,

    /// Add an existing directory to the sandbox's write allowlist for this
    /// invocation. Repeat the flag to approve multiple project roots.
    #[arg(long = "add-dir", value_name = "PATH")]
    add_dirs: Vec<PathBuf>,

    /// Use a provider from $LAZAR_HOME/providers.toml (see `lazar providers`).
    /// Authoritative for endpoint and API key; --model still overrides the
    /// model. Also settable as LAZAR_PROVIDER.
    #[arg(long, value_name = "ID")]
    provider: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Speak Agent Client Protocol on stdio for streaming, multi-turn clients.
    Acp(acp::AcpArgs),
    /// List the provider catalog ($LAZAR_HOME/providers.toml). Never prints secrets.
    Providers {
        /// Emit JSON (one object per provider) for front ends.
        #[arg(long)]
        json: bool,
    },
    /// Running and recently finished sub-agent jobs (lazar-spawn) as JSON.
    Jobs {
        /// Print one job's record and timeline instead of the list.
        #[arg(long, value_name = "JOB_ID")]
        show: Option<String>,
        /// Include jobs finished within this many seconds (default 600).
        #[arg(long, value_name = "SECS")]
        since: Option<u64>,
    },
    /// List installed skills ($LAZAR_HOME/user/skills) for pickers.
    Skills {
        /// Emit JSON (one object per skill).
        #[arg(long)]
        json: bool,
    },
    /// Check verification claims (one per line on stdin, or a [VERIFY] block)
    /// against the filesystem. Prints JSON; exits 0 only when every claim holds.
    Verify {
        /// Only parse the claims; do not touch the filesystem.
        #[arg(long)]
        lint: bool,
    },
}

/// Which session-lifecycle hooks `run_agent` should fire.
///
/// The one-shot CLI owns start+end. The long-lived ACP server fires those
/// once per ACP session instead of once per prompt turn.
#[derive(Clone, Debug)]
pub(crate) struct AgentLifecycle {
    pub fire_session_start: bool,
    pub fire_session_end: bool,
    pub initial_context: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentOutcome {
    pub stop_reason: String,
}

impl Default for AgentLifecycle {
    fn default() -> Self {
        Self {
            fire_session_start: true,
            fire_session_end: true,
            initial_context: String::new(),
        }
    }
}

impl AgentLifecycle {
    pub(crate) fn acp_turn(initial_context: String) -> Self {
        Self {
            fire_session_start: false,
            fire_session_end: false,
            initial_context,
        }
    }
}

pub(crate) fn lazar_home() -> PathBuf {
    // Allow override via LAZAR_HOME for non-default install locations.
    if let Ok(p) = env::var("LAZAR_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    PathBuf::from(env::var("HOME").expect("HOME unset")).join("lazar")
}

/// Operator runtime: skills, memory, workspace, logs, and hooks.
/// This tree is never part of the published core.
pub(crate) fn user_root() -> PathBuf {
    lazar_home().join("user")
}

pub(crate) fn user_dir(name: &str) -> PathBuf {
    user_root().join(name)
}

/// Move a legacy top-level runtime directory into `user/` once.
/// Never leaves a symlink behind.
pub(crate) fn prepare_user_layout() {
    let home = lazar_home();
    let user = user_root();
    let _ = fs::create_dir_all(&user);
    for name in ["skills", "hooks", "memory", "workspace", "logs"] {
        let old = home.join(name);
        let new = user.join(name);
        migrate_path(&old, &new);
        let _ = fs::create_dir_all(&new);
    }
    for name in ["providers.toml", "access-policy.json", ".guardrails", ".env"] {
        migrate_path(&home.join(name), &user.join(name));
    }
}

fn migrate_path(old: &Path, new: &Path) {
    if new.exists() || !old.exists() {
        return;
    }
    let legacy = match fs::symlink_metadata(old) {
        Ok(meta) => meta,
        Err(_) => return,
    };
    if legacy.file_type().is_symlink() {
        return;
    }
    let _ = fs::rename(old, new);
}

fn reset_all(skip_confirm: bool) -> Result<(), Box<dyn std::error::Error>> {
    let home = lazar_home();
    validate_reset_home(&home)
        .map_err(|msg| std::io::Error::new(std::io::ErrorKind::InvalidInput, msg))?;

    if !skip_confirm {
        eprint!(
            "Wipe skills/, hooks/, memory/, workspace/, logs/, and saved access approvals in {} and reseed?\n[y/N] ",
            home.display()
        );
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim(), "y" | "Y" | "yes") {
            eprintln!("aborted.");
            return Ok(());
        }
    }

    prepare_user_layout();
    for sub in ["skills", "hooks", "memory", "workspace", "logs"] {
        let live = user_dir(sub);
        if live.exists() {
            fs::remove_dir_all(&live)?;
        }
        fs::create_dir_all(&live)?;
    }
    remove_access_policy(&user_root())?;
    remove_access_policy(&home)?;
    SEED_SKILLS.extract(user_dir("skills"))?;
    SEED_HOOKS.extract(user_dir("hooks"))?;
    eprintln!(
        "[lazar] reset complete. {} skill files + {} hook files written.",
        SEED_SKILLS.files().count(),
        SEED_HOOKS.files().count()
    );
    Ok(())
}

fn remove_access_policy(home: &Path) -> std::io::Result<()> {
    match fs::remove_file(home.join("access-policy.json")) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// First-run safety: if `~/lazar/hooks/` doesn't exist (e.g. user upgraded
/// from a pre-hooks version without re-running --reset-all), seed the
/// directory in place. Skills are NOT touched — only the missing tree.
pub(crate) fn ensure_hooks_seeded() {
    let hooks = user_dir("hooks");
    if hooks.exists() {
        return;
    }
    if let Err(e) = fs::create_dir_all(&hooks) {
        eprintln!("[lazar] WARN: could not create {}: {e}", hooks.display());
        return;
    }
    if let Err(e) = SEED_HOOKS.extract(&hooks) {
        eprintln!("[lazar] WARN: could not seed hooks tree at {}: {e}", hooks.display());
        return;
    }
    eprintln!(
        "[lazar] seeded {} into {} (first-run, was missing)",
        SEED_HOOKS.files().count(),
        hooks.display()
    );
}

pub(crate) fn read_capped<R>(mut reader: R, cap: usize) -> thread::JoinHandle<CapturedOutput>
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
pub(crate) fn kill_process_group(pid: u32) {
    if pid <= 1 {
        return;
    }
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_process_group(_pid: u32) {}

/// Sandbox is off unless the operator opts in with `--sandbox`,
/// `LAZAR_SANDBOX=1`, or a `.guardrails` file that says `on`.
/// `LAZAR_NO_SANDBOX=1` forces it off.
pub(crate) fn no_sandbox_active() -> bool {
    if env_flag_enabled("LAZAR_NO_SANDBOX") {
        return true;
    }
    if env_flag_enabled("LAZAR_SANDBOX") {
        return false;
    }
    persisted_guardrails_off(&lazar_home()).unwrap_or(true)
}

struct ToolExecutionResult {
    output: String,
    exit_code: i32,
}

impl ToolExecutionResult {
    fn failure(output: String) -> Self {
        Self { output, exit_code: 1 }
    }
}

fn finalize_tool_output(output: &str, exit_code: i32, max_bytes: usize) -> String {
    let bounded = truncate_tool_output(output, max_bytes);
    format!("{bounded}\n[exit {exit_code}]")
}

fn run_bash(cmd: &str) -> ToolExecutionResult {
    if acp::is_session_cancelled() {
        return ToolExecutionResult { output: "cancelled".into(), exit_code: 130 };
    }
    let limits = ToolLimits::from_env();
    let lazar_path = lazar_home();
    let lazar = lazar_path.to_string_lossy().to_string();
    let workspace = format!("{lazar}/workspace");

    if let Err(e) = fs::create_dir_all(&workspace) {
        return ToolExecutionResult::failure(format!("[workspace error: {e}]\n[exit 1]"));
    }

    let current_depth = env::var("LAZAR_DEPTH")
        .ok()
        .and_then(|d| d.parse::<u32>().ok())
        .unwrap_or(0);
    let child_depth = current_depth.saturating_add(1);

    let policy = access::current();
    let unsandboxed = policy.unsandboxed();
    let mut command = spawn::bash_command(spawn::BashPayload::Inline(cmd), &lazar, &policy);

    // When unsandboxed, use the operator's real HOME (so things like ~/.ssh,
    // git config, gh CLI, claude/codex auth all work without redirection).
    // When sandboxed, HOME is pinned to LAZAR_HOME (existing behavior).
    let bash_home = spawn::bash_home(&lazar, unsandboxed);

    command
        .current_dir(policy.tool_cwd())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    spawn::apply_base_env(&mut command, &lazar, &bash_home, &policy);
    command
        .env("LAZAR_TOOL_ENV", "1")
        .env("LAZAR_DEPTH", child_depth.to_string());

    // Propagate the one-shot CLI or ACP worker session so child helpers can
    // enforce their documented per-session limits.
    if let Some(session_id) = core::invocation::session_id() {
        if session::validate_session_id(&session_id).is_ok() {
            command.env("LAZAR_SESSION_ID", session_id);
        }
    }

    // Propagate LAZAR_NO_SANDBOX so nested `lazar -p` calls inherit the
    // mode (otherwise the child would re-sandbox and the operator's
    // intent would be silently dropped). Handled by apply_base_env above
    // (sets LAZAR_NO_SANDBOX + PASSTHROUGH_VARS when unsandboxed).

    if let Ok(model) = env::var("LAZAR_MODEL") {
        command.env("LAZAR_MODEL", model);
    }
    if let Ok(provider) = env::var("LAZAR_PROVIDER") {
        command.env("LAZAR_PROVIDER", provider);
    }
    for budget in ["LAZAR_MAX_TURNS", "LAZAR_MAX_TOOL_CALLS", "LAZAR_MAX_TOKENS", "LAZAR_STREAM_IDLE_SECS"] {
        if let Ok(value) = env::var(budget) {
            command.env(budget, value);
        }
    }

    if env_flag_enabled(INHERIT_PROVIDER_KEY_ENV)
        || env_flag_enabled(LEGACY_INHERIT_PROVIDER_KEY_ENV)
    {
        // This is an operator opt-in on the parent Lazar process. Expose only
        // the credential used by this invocation, not every catalog key.
        command
            .env(INHERIT_PROVIDER_KEY_ENV, "1")
            .env(LEGACY_INHERIT_PROVIDER_KEY_ENV, "1");
        if let Some((name, value)) = active_provider_credential(current_depth) {
            command.env(name, value);
        }
        if env_non_empty("LAZAR_PROVIDER").is_none() {
            if let Some(base_url) = env_non_empty("ANTHROPIC_BASE_URL") {
                command.env("ANTHROPIC_BASE_URL", base_url);
            }
        }
    }

    let (mut child, _owner) = match spawn::spawn_owned(command) {
        Ok(child) => child,
        Err(e) => return ToolExecutionResult::failure(format!("[spawn error: {e}]\n[exit 1]")),
    };
    let child_pid = child.id();

    let stdout = child
        .stdout
        .take()
        .map(|out| read_capped(out, limits.output_max_bytes));
    let stderr = child
        .stderr
        .take()
        .map(|err| read_capped(err, limits.output_max_bytes));

    let (status, timed_out) = match spawn::wait_with_timeout(
        &mut child,
        child_pid,
        limits.timeout,
        Duration::from_millis(50),
    ) {
        Ok(pair) => pair,
        Err(e) => return ToolExecutionResult::failure(format!("[wait error: {e}]\n[exit 1]")),
    };

    // The shell can exit while a background descendant still owns stdout or
    // stderr. Those inherited descriptors otherwise make the reader joins
    // block forever after a seemingly successful tool call. A tool call owns
    // its whole process group, so terminate remaining descendants before
    // draining the pipes.
    kill_process_group(child_pid);

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
            "\n[stdout truncated: {} bytes omitted; re-run a narrower command such as head, tail, or sed -n to read the rest]",
            stdout.total_bytes.saturating_sub(stdout.bytes.len())
        ));
    }

    if !stderr.bytes.is_empty() || stderr.was_truncated() {
        let err = String::from_utf8_lossy(&stderr.bytes);
        result.push_str("\n[stderr]\n");
        result.push_str(&err);
        if stderr.was_truncated() {
            result.push_str(&format!(
                "\n[stderr truncated: {} bytes omitted; re-run a narrower command such as head, tail, or sed -n to read the rest]",
                stderr.total_bytes.saturating_sub(stderr.bytes.len())
            ));
        }
    }

    if timed_out {
        result.push_str(&format!("\n[timeout after {}s]", limits.timeout.as_secs()));
    }
    let exit_code = status.and_then(|s| s.code()).unwrap_or(-1);
    ToolExecutionResult {
        output: finalize_tool_output(&result, exit_code, limits.output_max_bytes),
        exit_code,
    }
}

/// Credential to expose to a tool subprocess after an explicit operator
/// opt-in. Catalog selection is authoritative; without it, mirror the legacy
/// Anthropic-then-MiniMax resolution order. Keychain access remains depth-zero
/// only, while a key inherited into a nested Lazar stays available in env.
fn active_provider_credential(current_depth: u32) -> Option<(String, String)> {
    if let Some(id) = env_non_empty("LAZAR_PROVIDER") {
        let catalog = providers::Catalog::load(&lazar_home()).ok()?;
        let keys = providers::EnvThenKeychain {
            allow_keychain: current_depth == 0,
        };
        let resolved = providers::resolve(&catalog, &id, &keys).ok()?;
        return resolved.api_key.map(|key| (resolved.key_env, key));
    }

    env_non_empty("ANTHROPIC_API_KEY")
        .map(|key| ("ANTHROPIC_API_KEY".to_string(), key))
        .or_else(|| {
            env_non_empty("MINIMAX_API_KEY")
                .map(|key| ("MINIMAX_API_KEY".to_string(), key))
        })
}

thread_local! {
    /// Model id the provider reported in the latest `message_start`. Routers
    /// such as an `auto` model name the model that actually answered here.
    static SERVED_MODEL: std::cell::RefCell<Option<String>> = const {
        std::cell::RefCell::new(None)
    };
    static EVENT_SINK: std::cell::RefCell<Option<UnboundedSender<Value>>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Redirect stream-json events to a sink instead of stdout.
/// ACP owns stdout (newline-delimited JSON-RPC) so the agent loop must not print.
pub(crate) fn set_event_sink(tx: Option<UnboundedSender<Value>>) {
    EVENT_SINK.with(|slot| *slot.borrow_mut() = tx);
}

/// Emit a single JSON event to stdout (used when --format=json). Adds ts_ms.
/// This is the "structured streaming" output for programmatic consumers like
/// the TUI. Distinct from the canonical log at logs/stream.jsonl.
///
/// When an ACP event sink is installed, the event is forwarded there and
/// stdout is left untouched.
fn emit_event(mut event: Value) {
    let ts_ms = now_millis();
    if let Some(obj) = event.as_object_mut() {
        obj.insert("ts_ms".into(), json!(ts_ms));
    }
    if let Some(tx) = EVENT_SINK.with(|slot| slot.borrow().clone()) {
        let _ = tx.unbounded_send(event);
        return;
    }
    println!("{event}");
    let _ = std::io::stdout().flush();
}

/// Copy whichever usage counters a `message_start` / `message_delta` event
/// carries. Later events win; absent fields leave the running value alone.
fn absorb_usage(usage: &Value, input_tokens: &mut u64, output_tokens: &mut u64, cache_read_tokens: &mut u64) {
    // `input_tokens` is the cache-miss slice. `cache_read_input_tokens` is
    // counted beside it and is never added in.
    if let Some(n) = usage.get("input_tokens").and_then(Value::as_u64) {
        *input_tokens = n;
    }
    if let Some(n) = usage.get("output_tokens").and_then(Value::as_u64) {
        *output_tokens = n;
    }
    if let Some(n) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) {
        *cache_read_tokens = n;
    }
}

type ParsedSse = (Value, String, u64, u64, u64);

#[derive(Debug)]
struct ModelError {
    message: String,
    partial_text: Vec<Value>,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModelError {}

/// Incremental Anthropic SSE decoder shared by blocking fixture tests and the
/// cancellable async HTTP transport.
#[derive(Default)]
struct SseMessage {
    blocks: std::collections::HashMap<u64, Value>,
    open_tool_blocks: std::collections::HashSet<u64>,
    tool_input_buffers: std::collections::HashMap<u64, String>,
    text_buffers: std::collections::HashMap<u64, String>,
    stop_reason: String,
    printed_any_text: bool,
    saw_message_stop: bool,
    usage_input_tokens: u64,
    usage_output_tokens: u64,
    usage_cache_read_tokens: u64,
    response_bytes: usize,
    /// Aggregate response ceiling; 0 means the default output budget's.
    max_response_bytes: usize,
    saw_content_frame: bool,
    /// Stream-json text held until the coalesce window elapses or the block ends.
    pending_deltas: std::collections::HashMap<u64, String>,
    pending_since: Option<Instant>,
}

impl SseMessage {
    fn account(&mut self, bytes: usize) -> Result<(), Box<dyn std::error::Error>> {
        self.response_bytes = self.response_bytes.saturating_add(bytes);
        let limit = if self.max_response_bytes == 0 {
            core::context::max_response_bytes(core::context::MAX_OUTPUT_TOKENS)
        } else {
            self.max_response_bytes
        };
        if self.response_bytes > limit {
            return Err(format!(
                "model response exceeded the aggregate limit ({} > {limit} bytes)",
                self.response_bytes,
            )
            .into());
        }
        Ok(())
    }

    /// Consume one complete transport line. Returns true at `message_stop`.
    fn line(
        &mut self,
        line: &str,
        format: OutputFormat,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let line = line.trim_end_matches(['\n', '\r']);
        if line.starts_with("data:") {
            self.saw_content_frame = true;
        }
        // Anthropic sends `data: {...}` (space after colon). Kimi Code / Allegro
        // sends `data:{...}` (no space). Accept both.
        let data = if let Some(data) = line.strip_prefix("data: ") {
            data
        } else if let Some(data) = line.strip_prefix("data:") {
            data
        } else {
            return Ok(false);
        };
        if data.is_empty() {
            return Ok(false);
        }

        let v: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(_) => return Ok(false),
        };

        match v["type"].as_str().unwrap_or("") {
            "content_block_start" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                if !self.blocks.contains_key(&idx)
                    && self.blocks.len() >= core::context::MAX_MODEL_CONTENT_BLOCKS
                {
                    return Err(format!(
                        "model response exceeded {} content blocks",
                        core::context::MAX_MODEL_CONTENT_BLOCKS
                    )
                    .into());
                }
                let block = v["content_block"].clone();
                let initial_bytes = serde_json::to_vec(&block)?.len();
                if initial_bytes > core::context::MAX_CONTEXT_ITEM_BYTES {
                    return Err(format!(
                        "model content block exceeded the per-item limit ({initial_bytes} > {} bytes)",
                        core::context::MAX_CONTEXT_ITEM_BYTES
                    )
                    .into());
                }
                self.account(initial_bytes)?;
                if block["type"] == "thinking" && format == OutputFormat::StreamJson {
                    emit_event(json!({"type": "phase", "phase": "reasoning"}));
                }
                if block["type"] == "tool_use" {
                    self.open_tool_blocks.insert(idx);
                }
                if block["type"] == "text" {
                    if let Some(text) = block["text"].as_str().filter(|text| !text.is_empty()) {
                        self.text_buffers.insert(idx, text.to_string());
                    }
                }
                self.blocks.insert(idx, block);
            }
            "content_block_delta" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                if !self.blocks.contains_key(&idx) {
                    return Err(format!("content delta arrived before block {idx} started").into());
                }
                let delta = &v["delta"];
                match delta["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or("");
                        self.account(text.len())?;
                        core::context::push_bounded(
                            self.text_buffers.entry(idx).or_default(),
                            text,
                            "assistant text block",
                        )?;
                        match format {
                            OutputFormat::Text => {
                                print!("{text}");
                                let _ = std::io::stdout().flush();
                                self.printed_any_text = true;
                            }
                            OutputFormat::StreamJson => self.push_text_delta(idx, text),
                            OutputFormat::Json => {
                                // Suppress live output; final result emitted at end_turn.
                            }
                        }
                    }
                    "input_json_delta" => {
                        let partial = delta["partial_json"].as_str().unwrap_or("");
                        self.account(partial.len())?;
                        core::context::push_bounded(
                            self.tool_input_buffers.entry(idx).or_default(),
                            partial,
                            "tool input block",
                        )?;
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                if let Some(block) = self.blocks.get_mut(&idx) {
                    if block["type"] == "tool_use" {
                        self.open_tool_blocks.remove(&idx);
                        let buf = self.tool_input_buffers.remove(&idx).unwrap_or_default();
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
                    } else if block["type"] == "text" {
                        if let Some(buf) = self.text_buffers.remove(&idx) {
                            block["text"] = json!(buf);
                        }
                        if format == OutputFormat::StreamJson {
                            self.flush_text_delta(idx);
                            emit_event(json!({
                                "type": "text_done",
                                "index": idx,
                            }));
                        }
                    }
                }
            }
            "message_start" => {
                if let Some(served) = v["message"]["model"].as_str().filter(|m| !m.is_empty()) {
                    SERVED_MODEL.with(|slot| *slot.borrow_mut() = Some(served.to_string()));
                }
                // Anthropic reports input_tokens here; message_delta usually
                // carries only the (cumulative) output_tokens.
                if let Some(u) = v["message"].get("usage") {
                    absorb_usage(
                        u,
                        &mut self.usage_input_tokens,
                        &mut self.usage_output_tokens,
                        &mut self.usage_cache_read_tokens,
                    );
                }
            }
            "message_delta" => {
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    self.stop_reason = r.to_string();
                }
                if let Some(u) = v.get("usage") {
                    absorb_usage(
                        u,
                        &mut self.usage_input_tokens,
                        &mut self.usage_output_tokens,
                        &mut self.usage_cache_read_tokens,
                    );
                }
            }
            "message_stop" => {
                self.saw_message_stop = true;
                return Ok(true);
            }
            "error" => {
                let msg = core::context::truncate_item(
                    v["error"]["message"].as_str().unwrap_or("unknown"),
                );
                return Err(format!("stream error: {msg}").into());
            }
            _ => {}
        }
        Ok(false)
    }

    fn push_text_delta(&mut self, idx: u64, text: &str) {
        if text.is_empty() {
            return;
        }
        if self
            .pending_since
            .is_some_and(|started| started.elapsed() >= Duration::from_millis(60))
        {
            self.flush_text_deltas();
        }
        self.pending_deltas.entry(idx).or_default().push_str(text);
        if self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }
    }

    fn flush_text_deltas(&mut self) {
        let mut indices: Vec<u64> = self.pending_deltas.keys().copied().collect();
        indices.sort_unstable();
        for idx in indices {
            self.flush_text_delta(idx);
        }
    }

    fn flush_text_delta(&mut self, idx: u64) {
        let Some(text) = self.pending_deltas.remove(&idx) else {
            return;
        };
        if self.pending_deltas.is_empty() {
            self.pending_since = None;
        }
        if text.is_empty() {
            return;
        }
        emit_event(json!({
            "type": "text_delta",
            "index": idx,
            "text": text,
        }));
    }

    fn partial_text_blocks(&mut self) -> Vec<Value> {
        for (idx, buf) in self.text_buffers.drain() {
            if let Some(block) = self.blocks.get_mut(&idx) {
                if block["type"] == "text" {
                    block["text"] = json!(buf);
                }
            }
        }
        let mut indices: Vec<u64> = self.blocks.keys().copied().collect();
        indices.sort_unstable();
        indices
            .into_iter()
            .filter_map(|idx| {
                let block = self.blocks.get(&idx)?;
                (block["type"] == "text" && !self.open_tool_blocks.contains(&idx))
                    .then(|| block.clone())
            })
            .collect()
    }

    fn cut(mut self, message: String) -> Box<ModelError> {
        self.flush_text_deltas();
        Box::new(ModelError {
            message,
            partial_text: self.partial_text_blocks(),
        })
    }

    fn finish(mut self, format: OutputFormat) -> Result<ParsedSse, Box<dyn std::error::Error>> {
        if !self.open_tool_blocks.is_empty() {
            let mut indices: Vec<u64> = self.open_tool_blocks.iter().copied().collect();
            indices.sort_unstable();
            return Err(self.cut(format!(
                "SSE stream ended before content_block_stop for tool_use block(s) {indices:?}; refusing incomplete tool input"
            )));
        }
        if !self.saw_message_stop {
            // Some upstreams occasionally close the SSE socket without a final
            // `message_stop`. A prior stop reason makes that recoverable; no
            // stop reason means the response was interrupted mid-stream.
            if self.stop_reason.is_empty() {
                let blocks = self.blocks.len();
                return Err(self.cut(format!(
                    "SSE stream ended before message_stop and no stop_reason was set; partial content had {blocks} block(s)"
                )));
            }
            eprintln!(
                "[lazar] WARN: SSE stream ended before message_stop; recovering with stop_reason={:?}, {} block(s) already accumulated",
                self.stop_reason,
                self.blocks.len()
            );
        }

        self.flush_text_deltas();
        if format == OutputFormat::Text && self.printed_any_text {
            println!();
        }

        // Early-EOF recovery: fold text that never saw content_block_stop.
        for (idx, buf) in self.text_buffers.drain() {
            if let Some(block) = self.blocks.get_mut(&idx) {
                if block["type"] == "text" {
                    block["text"] = json!(buf);
                }
            }
        }

        let mut indices: Vec<u64> = self.blocks.keys().copied().collect();
        indices.sort();
        let content: Vec<Value> = indices
            .iter()
            .filter_map(|i| self.blocks.get(i).cloned())
            .collect();

        Ok((
            json!(content),
            self.stop_reason,
            self.usage_input_tokens,
            self.usage_output_tokens,
            self.usage_cache_read_tokens,
        ))
    }
}

#[cfg(test)]
fn parse_sse_reader<R: BufRead>(
    mut reader: R,
    format: OutputFormat,
) -> Result<ParsedSse, Box<dyn std::error::Error>> {
    let mut message = SseMessage::default();
    loop {
        if acp::is_session_cancelled() {
            return Err("cancelled".into());
        }
        let mut line = String::new();
        let bytes_read = reader
            .by_ref()
            .take((MAX_SSE_LINE_BYTES + 1) as u64)
            .read_line(&mut line)?;
        if bytes_read == 0 {
            break;
        }
        if bytes_read > MAX_SSE_LINE_BYTES {
            return Err(format!(
                "SSE event exceeded the transport limit ({bytes_read} > {MAX_SSE_LINE_BYTES} bytes)"
            )
            .into());
        }
        if message.line(&line, format)? {
            break;
        }
    }
    message.finish(format)
}

async fn wait_for_session_cancel() {
    loop {
        if acp::is_session_cancelled() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn feed_sse_chunk(
    message: &mut SseMessage,
    pending: &mut Vec<u8>,
    chunk: &[u8],
    format: OutputFormat,
) -> Result<bool, Box<dyn std::error::Error>> {
    for &byte in chunk {
        if byte == b'\n' {
            if acp::is_session_cancelled() {
                return Err("cancelled".into());
            }
            if pending.len().saturating_add(1) > MAX_SSE_LINE_BYTES {
                return Err(format!(
                    "SSE event exceeded the transport limit (> {MAX_SSE_LINE_BYTES} bytes)"
                )
                .into());
            }
            let line = std::str::from_utf8(pending)?;
            let stopped = message.line(line, format)?;
            pending.clear();
            if stopped {
                return Ok(true);
            }
        } else {
            if pending.len().saturating_add(1) >= MAX_SSE_LINE_BYTES {
                return Err(format!(
                    "SSE event exceeded the transport limit (> {MAX_SSE_LINE_BYTES} bytes)"
                )
                .into());
            }
            pending.push(byte);
        }
    }
    Ok(false)
}

fn stream_idle() -> Duration {
    env::var("LAZAR_STREAM_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|secs: &u64| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(120))
}

fn idle_stream_error(idle: Duration) -> String {
    format!(
        "provider stream idle during response body; no content frame for {}s",
        idle.as_secs()
    )
}

async fn parse_sse_response(
    mut response: reqwest::Response,
    format: OutputFormat,
    max_response_bytes: usize,
) -> Result<ParsedSse, Box<dyn std::error::Error>> {
    let mut message = SseMessage {
        max_response_bytes,
        ..SseMessage::default()
    };
    let mut pending = Vec::new();
    let idle = stream_idle();
    let mut last_content = Instant::now();

    loop {
        let waited = last_content.elapsed();
        if waited >= idle {
            return Err(message.cut(idle_stream_error(idle)));
        }
        let coalesce_left = message.pending_since.map(|started| {
            Duration::from_millis(60).saturating_sub(started.elapsed())
        });
        let flush_pending = coalesce_left.is_some();
        let coalesce_left = coalesce_left.unwrap_or(Duration::from_millis(60));
        let chunk = tokio::select! {
            biased;
            _ = wait_for_session_cancel() => {
                message.flush_text_deltas();
                return Err("cancelled".into());
            }
            _ = tokio::time::sleep(coalesce_left), if flush_pending => {
                message.flush_text_deltas();
                continue;
            }
            _ = tokio::time::sleep(idle.saturating_sub(waited)) => {
                return Err(message.cut(idle_stream_error(idle)));
            }
            chunk = response.chunk() => chunk?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        if feed_sse_chunk(&mut message, &mut pending, &chunk, format)? {
            break;
        }
        if message.saw_content_frame {
            last_content = Instant::now();
            message.saw_content_frame = false;
        }
    }
    if !pending.is_empty() {
        let line = std::str::from_utf8(&pending)?;
        let _ = message.line(line, format)?;
    }
    message.finish(format)
}

async fn response_preview(
    response: &mut reqwest::Response,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut kept = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;
    loop {
        let chunk = tokio::select! {
            biased;
            _ = wait_for_session_cancel() => return Err("cancelled".into()),
            chunk = response.chunk() => chunk?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        total = total.saturating_add(chunk.len());
        if kept.len() < core::context::MAX_CONTEXT_ITEM_BYTES {
            let remaining = core::context::MAX_CONTEXT_ITEM_BYTES - kept.len();
            kept.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        if total > core::context::MAX_CONTEXT_ITEM_BYTES {
            truncated = true;
            break;
        }
    }
    let mut preview = String::from_utf8_lossy(&kept).to_string();
    if truncated {
        preview.push_str("\n[response body truncated]");
    }
    Ok(preview)
}

async fn request_model(
    client: &reqwest::Client,
    api_url: &str,
    api_key: &str,
    body: &Value,
    format: OutputFormat,
) -> Result<ParsedSse, Box<dyn std::error::Error>> {
    let request = client
        .post(api_url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(body)
        .send();
    let mut response = tokio::select! {
        biased;
        _ = wait_for_session_cancel() => return Err("cancelled".into()),
        response = request => match response {
            Ok(response) => response,
            Err(error) => return Err(format!("API request failed: {error}").into()),
        },
    };
    if !response.status().is_success() {
        let status = response.status();
        let text = response_preview(&mut response).await?;
        return Err(format!("API {status}: {text}").into());
    }
    let max_output = body["max_tokens"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
    parse_sse_response(
        response,
        format,
        core::context::max_response_bytes(max_output.max(core::context::MAX_OUTPUT_TOKENS)),
    )
    .await
}

/// RAII guard that fires the `session-end` hook on drop. Defaults to
/// `status: "error"` so any early return (panic, ?, explicit Err) reports
/// as an error session. The success path calls `mark_ok()` before returning.
struct SessionEndGuard {
    depth: u32,
    base_payload: Value,
    status: std::cell::Cell<&'static str>,
    error: std::cell::RefCell<Option<String>>,
    fired: std::cell::Cell<bool>,
}

impl SessionEndGuard {
    fn new(depth: u32, base_payload: Value) -> Self {
        Self {
            depth,
            base_payload,
            status: std::cell::Cell::new("error"),
            error: std::cell::RefCell::new(None),
            fired: std::cell::Cell::new(false),
        }
    }
    fn mark_ok(&self) {
        self.status.set("ok");
    }
    fn mark_error(&self, err: impl Into<String>) {
        self.status.set("error");
        *self.error.borrow_mut() = Some(err.into());
    }
    fn suppress(&self) {
        self.fired.set(true);
    }
}

impl Drop for SessionEndGuard {
    fn drop(&mut self) {
        if self.fired.get() {
            return;
        }
        self.fired.set(true);
        // Only top-level invocations fire session-end. Nested lazar calls
        // are tools, not sessions.
        if self.depth != 0 {
            return;
        }
        let mut payload = self.base_payload.clone();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("status".into(), json!(self.status.get()));
            if let Some(err) = self.error.borrow().as_ref() {
                obj.insert("error".into(), json!(err));
            }
        }
        hooks::fire(HookEvent::SessionEnd, payload);
    }
}

/// Uniform abort: mirror the error to stream-json consumers, record
/// invoke_end in the canonical log, mark the session guard, and build the
/// Err. Every early-exit in the agent loop MUST go through this.
fn session_write_failed(
    format: OutputFormat,
    session_guard: &SessionEndGuard,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(err) = log::take_session_write_error() {
        return Err(abort_invoke(
            format,
            session_guard,
            "error",
            format!("session log write failed: {err}"),
        ));
    }
    Ok(())
}

fn abort_invoke(
    format: OutputFormat,
    session_guard: &SessionEndGuard,
    stop_reason: &str,
    msg: String,
) -> Box<dyn std::error::Error> {
    emit_stream_error(format, &msg);
    append_stream(json!({"kind": "invoke_end", "stop_reason": stop_reason, "note": &msg}));
    session_guard.mark_error(&msg);
    msg.into()
}

/// Provider endpoint, credential, and model for one invocation.
struct ProviderConfig {
    api_key: String,
    api_url: String,
    model: String,
    /// Catalog id when `--provider` / `LAZAR_PROVIDER` selected one.
    provider_id: Option<String>,
    /// Catalog context window in tokens, when the provider declares one.
    context_window: Option<u64>,
}

// Manual so a panic or log line can never print the key.
impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("api_key", &"<redacted>")
            .field("api_url", &self.api_url)
            .field("model", &self.model)
            .field("provider_id", &self.provider_id)
            .finish()
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Resolve the provider for this invocation.
///
/// A catalog provider (`--provider` / `LAZAR_PROVIDER`) is authoritative for
/// endpoint and key: an `ANTHROPIC_BASE_URL` / `ANTHROPIC_API_KEY` left in
/// the environment by an earlier launcher is residue, and honouring it was
/// the "switched to kimi, still billing MiniMax" bug. `--model` and
/// `LAZAR_MODEL` still override the provider's default model.
///
/// Without a selected provider the legacy env contract is unchanged:
/// `ANTHROPIC_API_KEY` wins and `MINIMAX_API_KEY` is the documented fallback
/// that also switches the endpoint and model to MiniMax's Anthropic-compatible
/// surface.
fn resolve_provider(model_override: Option<String>) -> Result<ProviderConfig, String> {
    let selected = match env_non_empty("LAZAR_PROVIDER") {
        Some(id) => {
            let depth: u32 = env::var("LAZAR_DEPTH")
                .ok()
                .and_then(|d| d.parse().ok())
                .unwrap_or(0);
            let catalog = providers::Catalog::load(&lazar_home())?;
            // A child started by the trusted lazar-spawn reads the Keychain
            // itself, so the key never has to sit in a tool's environment.
            let keys = providers::EnvThenKeychain {
                allow_keychain: depth == 0 || env::var("LAZAR_SPAWNED").as_deref() == Ok("1"),
            };
            Some(providers::resolve(&catalog, &id, &keys)?)
        }
        None => None,
    };

    let (api_key, base, is_minimax_fallback) = match &selected {
        Some(provider) => {
            let key = provider
                .api_key
                .clone()
                .ok_or_else(|| providers::missing_key_message(provider))?;
            (key, provider.base_url.clone(), false)
        }
        None => match env_non_empty("ANTHROPIC_API_KEY") {
            Some(key) => (
                key,
                env_non_empty("ANTHROPIC_BASE_URL")
                    .unwrap_or_else(|| "https://api.anthropic.com".to_string()),
                false,
            ),
            None => match env_non_empty("MINIMAX_API_KEY") {
                Some(key) => (
                    key,
                    env_non_empty("ANTHROPIC_BASE_URL")
                        .unwrap_or_else(|| "https://api.minimax.io/anthropic".to_string()),
                    true,
                ),
                None => {
                    return Err("ANTHROPIC_API_KEY is not set (nor MINIMAX_API_KEY); pass --provider <id> or set LAZAR_PROVIDER to use the catalog (see `lazar providers`)".into())
                }
            },
        },
    };
    let api_url = format!("{}/v1/messages", base.trim_end_matches('/'));
    let model = model_override
        .or_else(|| env_non_empty("LAZAR_MODEL"))
        .or_else(|| selected.as_ref().map(|p| p.default_model.clone()))
        .unwrap_or_else(|| {
            if is_minimax_fallback {
                "MiniMax-M3".to_string()
            } else {
                DEFAULT_MODEL.into()
            }
        });
    Ok(ProviderConfig {
        api_key,
        api_url,
        model,
        context_window: selected.as_ref().and_then(|p| p.context_window),
        provider_id: selected.map(|p| p.id),
    })
}

fn print_providers(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let catalog = providers::Catalog::load(&lazar_home())?;
    let keys = providers::EnvThenKeychain {
        allow_keychain: true,
    };
    let rows = providers::describe(&catalog, &keys);
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print!("{}", providers::render_table(&rows));
    }
    Ok(())
}

/// Per-invocation ceilings for the agent loop. All env-tunable; see the
/// README "Environment" section.
#[derive(Clone, Copy, Debug)]
struct LoopLimits {
    /// The agent loop lives HERE, in the kernel — never in a client. It
    /// trusts the provider's `stop_reason`: `tool_use` executes and continues,
    /// a terminal reason ends the turn. The kernel does not inspect the
    /// model's prose to decide whether it "really" finished.
    max_agent_turns: u32,
    /// Model output cap. The old hardcoded 8192 (~30KB) truncated large single
    /// writes mid-string → unparseable tool_use → "empty tool call" abort.
    /// Configurable, with a higher default so big files don't require
    /// chunking as aggressively. Env-tunable per model's real output ceiling.
    max_tokens: u32,
    max_tool_calls: u32,
}

impl LoopLimits {
    fn from_env() -> Self {
        fn positive(name: &str, default: u32) -> u32 {
            env::var(name)
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|n: &u32| *n > 0)
                .unwrap_or(default)
        }
        Self {
            max_agent_turns: positive("LAZAR_MAX_TURNS", DEFAULT_MAX_AGENT_TURNS),
            max_tokens: positive("LAZAR_MAX_TOKENS", DEFAULT_MAX_TOKENS),
            max_tool_calls: positive("LAZAR_MAX_TOOL_CALLS", DEFAULT_MAX_TOOL_CALLS),
        }
    }
}

/// Fire session-start / user-prompt hooks and join the context they inject.
/// Top-level only: nested lazar calls (depth > 0) act like tools, not new
/// sessions. The long-lived ACP server fires session-start itself, once per
/// ACP session, and opts out here via `lifecycle`.
fn collect_hook_context(
    depth: u32,
    lifecycle: AgentLifecycle,
    model: &str,
    prompt: &str,
    session_id: &Option<String>,
) -> String {
    let mut injected = lifecycle.initial_context;
    if depth == 0 && lifecycle.fire_session_start {
        let start_results = hooks::fire(
            HookEvent::SessionStart,
            json!({
                "depth": depth,
                "model": model,
                "prompt": prompt,
                "session_id": session_id,
            }),
        );
        injected.push_str(&hooks::join_injected_contexts(&start_results));
    }
    if depth == 0 {
        let prompt_results = hooks::fire(
            HookEvent::UserPrompt,
            json!({
                "depth": depth,
                "prompt": prompt,
            }),
        );
        let prompt_ctx = hooks::join_injected_contexts(&prompt_results);
        if !prompt_ctx.is_empty() {
            if !injected.is_empty() {
                injected.push_str("\n\n");
            }
            injected.push_str(&prompt_ctx);
        }
    }
    text::head_bytes(&injected, core::context::MAX_HOOK_CONTEXT_BYTES).to_string()
}

/// Render the system prompt: the compiled-in template, then hook-injected
/// runtime context, then the authoritative identity and access policy (which
/// override anything stale in memory or hook text).
fn build_system_prompt(
    depth: u32,
    injected_context: &str,
    model: &str,
    api_url: &str,
    provider: Option<&str>,
) -> Result<String, String> {
    let home = lazar_home();
    let skills = user_dir("skills");

    let mut system = SYSTEM_PROMPT_TEMPLATE
        .replace("{max_depth}", &MAX_DEPTH.to_string())
        .replace("{depth}", &depth.to_string())
        .replace("{home_disp}", &home.display().to_string())
        .replace("{skills_disp}", &skills.display().to_string())
        .replace(
            "{sandbox_status_line}",
            if no_sandbox_active() {
                "Current mode: SANDBOX OFF (default)."
            } else {
                "Current mode: sandboxed (--sandbox is active)."
            },
        );

    // Hooks that return {"action":"inject","context":"..."} on session-start
    // or user-prompt land here — project context, ambient facts about the
    // operator's environment, etc.
    system.push_str("\n\nRUNTIME CONTEXT (from hooks)\n");
    system.push_str(injected_context);
    system.push_str("\n\n");
    system.push_str(&runtime_identity_context(
        model,
        api_url,
        provider,
        acp::connected_client().as_ref(),
    ));
    system.push_str("\n\n");
    system.push_str(&access::current().prompt_context());
    core::context::validate_text("system prompt", &system)?;
    Ok(system)
}

/// The single tool the model gets. The description deliberately carries
/// strong shell-execution priors and the delegation rule the pre-tool hook
/// enforces.
fn bash_tool_schema() -> Value {
    json!({
        "name": "bash",
        "description": "Run a bash command and return stdout, stderr, and exit code. \
            This is a normal bash shell — pipelines, heredocs, find/xargs, awk/sed, \
            jq, redirection, all work as you'd expect. Read skills via `cat`. \
            Self-modify by writing files under skills/. Delegate to a sub-agent \
            only via `bash $LAZAR_HOME/bin/lazar-spawn` — raw `lazar -p` is vetoed.",
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
    })
}

/// Outcome of one `tool_use` block: the `tool_result` to send back plus the
/// bookkeeping the loop needs for its empty-turn and budget accounting.
struct ToolUseOutcome {
    result_event: Value,
    /// The block carried a non-empty command (even if a hook then vetoed it).
    valid_command: bool,
}

/// Run one bash tool_use block: pre-tool hooks (veto / rewrite), the
/// sandboxed shell, post-tool hooks (rewrite output), then record the result
/// in the canonical log and the stream-json feed.
fn execute_tool_use(block: &Value, depth: u32, verbose: bool, format: OutputFormat) -> ToolUseOutcome {
    let cmd_raw = block["input"]["command"].as_str().unwrap_or("");
    let cmd = cmd_raw.trim();
    let mut executed_command: Option<String> = None;
    let mut kernel_exit_code = 1_i32;

    let output = if command_hunts_credentials(cmd) {
        kernel_exit_code = 1;
        "[vetoed: reading stored credentials (Keychain, process environments, env dumps of keys/tokens) is blocked. If a key or token is missing, stop and tell the operator which one and where it is expected — do not search for it.]\n[exit 1]"
            .to_string()
    } else if command_invokes_raw_lazar(cmd) {
        kernel_exit_code = 1;
        "[vetoed: raw `lazar -p` spawns are disabled — delegate via locked bin: bash $LAZAR_HOME/bin/lazar-spawn <task-id> <prompt-file>]\n[exit 1]"
            .to_string()
    } else if cmd.is_empty() {
        // Refuse empty commands. Surface a clear error to the model instead
        // of running `bash -c ""` and getting [exit 0]. This prevents the
        // kernel from being a silent accomplice in tight loops of malformed
        // tool calls.
        if verbose {
            eprintln!("[lazar] tool_use: <EMPTY COMMAND — refused>");
        }
        "[error: 'bash' was called with an empty or missing 'command' field. \
         Provide a non-empty bash command, or end your turn instead of calling \
         the tool with no arguments.]\n[exit 1]"
            .to_string()
    } else {
        if verbose {
            let preview: String = cmd.chars().take(120).collect();
            eprintln!("[lazar] tool_use: {preview}");
        }

        // pre-tool hook: veto blocks the call, transform rewrites it.
        let pre = hooks::fire(
            HookEvent::PreTool,
            json!({
                "depth": depth,
                "tool_use_id": block["id"],
                "command": cmd_raw,
            }),
        );
        if let Some(reason) = hooks::first_veto(&pre) {
            if verbose {
                eprintln!("[lazar] pre-tool veto: {reason}");
            }
            format!(
                "[vetoed by hook: {reason}]\n[exit 1]\n\
                 [note: A pre-tool hook blocked this command. Adjust your approach \
                 or escalate to the user; do not retry the same command.]"
            )
        } else {
            let effective_cmd = hooks::last_transform_command(&pre)
                .map(|s| s.to_string())
                .unwrap_or_else(|| cmd.to_string());
            if verbose && effective_cmd != cmd {
                eprintln!("[lazar] pre-tool rewrote command");
            }
            executed_command = Some(effective_cmd.clone());
            let execution = run_bash(&effective_cmd);
            kernel_exit_code = execution.exit_code;
            let raw_output = execution.output;

            // post-tool hook: transform rewrites the output before it goes
            // back to the model.
            let post = hooks::fire(
                HookEvent::PostTool,
                json!({
                    "depth": depth,
                    "tool_use_id": block["id"],
                    "command": &effective_cmd,
                    "output": &raw_output,
                }),
            );
            hooks::last_transform_output(&post)
                .map(|s| s.to_string())
                .unwrap_or(raw_output)
        }
    };

    // A post-tool hook can replace a bounded command result with its own
    // output. Re-apply the kernel ceiling after every transform so the hook
    // cannot inflate the next model request.
    let output = truncate_tool_output(&output, ToolLimits::from_env().output_max_bytes);

    let result_event = json!({
        "type": "tool_result",
        "tool_use_id": block["id"],
        "content": output,
        "exit_code": kernel_exit_code,
    });
    append_stream(json!({
        "kind": "tool_result",
        "tool_use_id": block["id"],
        "command": cmd_raw,
        "requested_command": cmd_raw,
        "executed_command": executed_command.clone(),
        "content": result_event["content"],
        "exit_code": kernel_exit_code,
    }));
    if format == OutputFormat::StreamJson {
        emit_event(json!({
            "type": "tool_result",
            "tool_use_id": block["id"],
            "command": cmd_raw,
            "requested_command": cmd_raw,
            "executed_command": executed_command,
            "content": output,
            "exit_code": kernel_exit_code,
        }));
    }
    if verbose {
        eprintln!("[lazar] tool_result: {} bytes", output.len());
    }

    ToolUseOutcome {
        result_event,
        valid_command: !cmd.is_empty(),
    }
}

/// True when the command itself starts a nested `lazar -p` / `lazar --prompt`.
/// `lazar-spawn` is a different word and is allowed.
fn command_invokes_raw_lazar(command: &str) -> bool {
    let mut rest = command;
    while let Some(index) = rest.find("lazar") {
        let boundary_ok = index == 0
            || rest[..index]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_whitespace() || matches!(ch, '/' | ';'));
        let after = index + "lazar".len();
        let word_ok = rest[after..]
            .chars()
            .next()
            .is_none_or(|ch| ch.is_whitespace());
        if boundary_ok && word_ok && prompt_flag_in_tokens(&rest[after..]) {
            return true;
        }
        rest = &rest[after..];
    }
    false
}

/// Commands whose purpose is reading stored secrets: Keychain dumps, another
/// process's environment, or an env listing filtered for keys and tokens.
/// A model that hits a missing credential must report it, not go looking.
fn command_hunts_credentials(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let keychain = lower.contains("security ")
        && ["find-generic-password", "find-internet-password", "dump-keychain", "export -k"]
            .iter()
            .any(|verb| lower.contains(verb));
    let process_env = lower.contains("/proc/") && lower.contains("environ");
    let squashed: String = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    let lists_env = ["printenv", "env |", "env|", "export -p", "set |", "declare -p", "declare -x"]
        .iter()
        .any(|listing| squashed.contains(listing));
    let filters_secrets = lower.contains("grep")
        && ["key", "token", "secret", "passw", "credential"]
            .iter()
            .any(|needle| lower.contains(needle));
    keychain || process_env || (lists_env && filters_secrets)
}

fn prompt_flag_in_tokens(after_name: &str) -> bool {
    after_name.split_whitespace().any(|token| {
        let token = token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | ';' | '&' | '|'));
        token == "-p" || token == "--prompt" || token.starts_with("--prompt=") || token.starts_with("-p=")
    })
}

fn request_body(
    model: &str,
    max_tokens: u32,
    system: &str,
    tool: &Value,
    messages: &[Value],
) -> Value {
    let mut messages = messages.to_vec();
    for message in &mut messages {
        if let Some(blocks) = message["content"].as_array_mut() {
            for block in blocks {
                if block["type"] == "tool_result" {
                    if let Some(obj) = block.as_object_mut() {
                        if obj.get("exit_code").and_then(Value::as_i64).is_some_and(|code| code != 0) {
                            obj.insert("is_error".into(), json!(true));
                        }
                        obj.remove("exit_code");
                    }
                }
            }
        }
    }
    json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "tools": [tool],
        "messages": messages,
        "stream": true,
    })
}

/// agent-stop hook, top level only.
fn fire_agent_stop(depth: u32, stop_reason: &str, duration_ms: u128) {
    if depth != 0 {
        return;
    }
    hooks::fire(
        HookEvent::AgentStop,
        json!({
            "depth": depth,
            "stop_reason": stop_reason,
            "duration_ms": duration_ms,
        }),
    );
}

pub(crate) fn run_agent(
    prompt: &str,
    format: OutputFormat,
    verbose: bool,
    model_override: Option<String>,
    lifecycle: AgentLifecycle,
) -> Result<AgentOutcome, Box<dyn std::error::Error>> {
    let started = Instant::now();
    if let Err(msg) = core::context::validate(&core::context::OperatorPrompt(prompt)) {
        emit_stream_error(format, &msg);
        return Err(msg.into());
    }
    let provider = match resolve_provider(model_override) {
        Ok(provider) => provider,
        Err(msg) => {
            emit_stream_error(format, &msg);
            return Err(msg.into());
        }
    };
    let ProviderConfig {
        api_key,
        api_url,
        model,
        provider_id,
        context_window,
    } = provider;
    let limits = LoopLimits::from_env();
    let window = core::context::Window::resolve(context_window, limits.max_tokens);
    let depth: u32 = env::var("LAZAR_DEPTH")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0);

    if depth > MAX_DEPTH {
        let msg = format!("recursion depth {depth} exceeds max {MAX_DEPTH}");
        emit_stream_error(format, &msg);
        return Err(msg.into());
    }

    // One-shot CLI sessions use the env contract; long-lived ACP workers use
    // a thread-local scope so concurrent lifecycle work cannot leak.
    let session_id = core::invocation::session_id();

    // session-end fires on every exit path via Drop. Top-level only.
    let session_guard = SessionEndGuard::new(
        depth,
        json!({
            "depth": depth,
            "model": &model,
            "prompt": prompt,
            "session_id": session_id,
        }),
    );
    if !lifecycle.fire_session_end {
        session_guard.suppress();
    }
    let _ = log::take_session_write_error();

    let injected_context = collect_hook_context(depth, lifecycle, &model, prompt, &session_id);
    let system = match build_system_prompt(
        depth,
        &injected_context,
        &model,
        &api_url,
        provider_id.as_deref(),
    ) {
        Ok(system) if core::answer::enabled() => format!("{system}\n\n{}", core::answer::PROMPT_SECTION),
        Ok(system) => system,
        Err(msg) => return Err(abort_invoke(format, &session_guard, "error", msg)),
    };
    let tool = bash_tool_schema();

    // Load prior messages from the session log if --session is set. Loaded
    // messages are guaranteed valid: start with user-role, alternating,
    // under byte cap.
    let mut messages: Vec<Value> = Vec::new();
    if let Some(ref id) = session_id {
        if depth == 0 {
            let prior = match session::load_messages_checked(id, window.history) {
                Ok(prior) => prior,
                Err(error) => return Err(abort_invoke(format, &session_guard, "context_limit", error)),
            };
            if verbose && !prior.is_empty() {
                eprintln!(
                    "[lazar] session={id} loaded {} prior messages",
                    prior.len()
                );
            }
            append_stream(json!({
                "kind": "session_continuation",
                "session_id": id,
                "loaded_messages": prior.len(),
            }));
            messages.extend(prior);
        }
    }
    // A leading `[skill:name]` / `/name` directive is resolved HERE, by the
    // kernel, against skills/<name>/SKILL.md. The model receives the skill's
    // instructions inline instead of a convention it has to decode and a
    // file it has to go and read. An unknown skill is the client's mistake
    // and is reported to the client; the model is never asked to guess.
    let skill = match skill::resolve(prompt, &user_dir("skills")) {
        Ok(skill) => skill,
        Err(msg) => {
            append_stream(json!({"kind": "user", "content": prompt}));
            return Err(abort_invoke(format, &session_guard, "error", msg));
        }
    };
    let model_prompt = skill
        .as_ref()
        .map(skill::SkillInvocation::prompt)
        .unwrap_or_else(|| prompt.to_string());
    if let Err(msg) =
        core::context::validate(&core::context::ExpandedSkillPrompt(&model_prompt))
    {
        append_stream(json!({"kind": "user", "content": prompt}));
        return Err(abort_invoke(format, &session_guard, "error", msg));
    }
    messages.push(json!({"role": "user", "content": &model_prompt}));

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .tcp_keepalive(Duration::from_secs(30))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            let msg = format!("reqwest client build failed: {e}");
            emit_stream_error(format, &msg);
            return Err(msg.into());
        }
    };
    let http_runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let msg = format!("async HTTP runtime build failed: {error}");
            emit_stream_error(format, &msg);
            return Err(msg.into());
        }
    };

    let acp_client = acp::connected_client().map(|c| c.to_json());
    append_stream(json!({
        "kind": "invoke_start",
        "depth": depth,
        "model": model,
        "provider": provider_id,
        "client": acp_client,
    }));
    if skill.is_some() {
        append_stream(json!({
            "kind": "user",
            "content": prompt,
            "model_content": &model_prompt,
        }));
    } else {
        append_stream(json!({"kind": "user", "content": prompt}));
    }
    session_write_failed(format, &session_guard)?;

    if format == OutputFormat::StreamJson {
        emit_event(json!({
            "type": "invoke_start",
            "depth": depth,
            "model": &model,
            "provider": &provider_id,
            "client": acp_client,
            "prompt": prompt,
        }));
    }
    if let Some(inv) = &skill {
        let event = json!({
            "name": inv.name,
            "path": inv.path,
            "bytes": inv.body.len(),
            "truncated": inv.truncated,
        });
        let mut logged = event.clone();
        logged["kind"] = json!("skill_inject");
        append_stream(logged);
        if format == OutputFormat::StreamJson {
            let mut emitted = event;
            emitted["type"] = json!("skill_inject");
            emit_event(emitted);
        }
        if verbose {
            eprintln!(
                "[lazar] skill `{}` injected from {} ({} bytes)",
                inv.name,
                inv.path.display(),
                inv.body.len()
            );
        }
    }

    if verbose {
        eprintln!("[lazar] invoke_start depth={depth} model={model}");
    }

    // Tight-loop safeguard: if the model emits N consecutive turns where every
    // tool_use has an empty/missing command, abort. Without this the kernel
    // happily runs `bash -c ""` forever.
    let mut consecutive_empty_turns: u32 = 0;
    const MAX_CONSECUTIVE_EMPTY_TURNS: u32 = 3;
    let mut agent_turns: u32 = 0;
    let mut tool_calls: u32 = 0;
    let mut total_usage_in: u64 = 0;
    let mut total_usage_out: u64 = 0;
    let mut total_cache_read: u64 = 0;
    // One forced compaction per overflow the provider reports; reset by the
    // next successful request.
    let mut overflow_retried = false;
    let mut verify_attempts: u32 = 0;
    // What the final answer's verification concluded, for the answer record.
    let mut verify_summary = json!({"status": "none", "claims": 0});
    let summary_tokens = limits.max_tokens.min(4_096);
    let mut summarise = |transcript: &str| -> Option<String> {
        let body = json!({
            "model": &model,
            "max_tokens": summary_tokens,
            "system": core::compact::SUMMARY_SYSTEM_PROMPT,
            "messages": [{"role": "user", "content": transcript}],
            "stream": true,
        });
        let (content, ..) = http_runtime
            .block_on(request_model(&client, &api_url, &api_key, &body, OutputFormat::Json))
            .ok()?;
        let text = text::extract_text(&content);
        (!text.trim().is_empty()).then_some(text)
    };

    loop {
        if acp::is_session_cancelled() {
            return Err(abort_invoke(
                format,
                &session_guard,
                "cancelled",
                "cancelled".into(),
            ));
        }
        agent_turns = agent_turns.saturating_add(1);
        if agent_turns > limits.max_agent_turns {
            let msg = format!(
                "aborted after reaching LAZAR_MAX_TURNS={}",
                limits.max_agent_turns
            );
            return Err(abort_invoke(format, &session_guard, "aborted", msg));
        }

        let mut body = request_body(&model, limits.max_tokens, &system, &tool, &messages);
        let body_len = body.to_string().len();
        if body_len > window.compact_at {
            let overhead = body_len.saturating_sub(core::compact::bytes(&messages));
            let plan = CompactPlan {
                ceiling: window.compact_at.saturating_sub(overhead),
                target: window.compact_to.saturating_sub(overhead),
            };
            if let Err(msg) = compact_messages(&mut messages, plan, &mut summarise, format) {
                return Err(abort_invoke(format, &session_guard, "context_limit", msg));
            }
            body = request_body(&model, limits.max_tokens, &system, &tool, &messages);
        }

        emit_phase(format, "thinking");
        let (content, stop_reason_str, usage_in, usage_out, cache_read) =
            match http_runtime.block_on(request_model(
                &client,
                &api_url,
                &api_key,
                &body,
                format,
            )) {
                Ok(parsed) => parsed,
                Err(error) if !overflow_retried && is_context_overflow(&error.to_string()) => {
                    // The window in the catalog was optimistic for this
                    // model: halve what we send and try once more.
                    overflow_retried = true;
                    let current = core::compact::bytes(&messages);
                    let plan = CompactPlan { ceiling: current / 2, target: current / 3 };
                    if let Err(msg) = compact_messages(&mut messages, plan, &mut summarise, format) {
                        return Err(abort_invoke(format, &session_guard, "context_limit", msg));
                    }
                    continue;
                }
                Err(error) => {
                    let msg = error.to_string();
                    if let Some(cut) = error.downcast_ref::<ModelError>() {
                        if !cut.partial_text.is_empty() {
                            append_stream(json!({
                                "kind": "assistant",
                                "content": cut.partial_text,
                            }));
                        }
                    }
                    if verbose {
                        eprintln!("[lazar] {msg}");
                    }
                    session_write_failed(format, &session_guard)?;
                    return Err(abort_invoke(format, &session_guard, "error", msg));
                }
            };
        total_usage_in = total_usage_in.saturating_add(usage_in);
        total_usage_out = total_usage_out.saturating_add(usage_out);
        total_cache_read = total_cache_read.saturating_add(cache_read);
        if format == OutputFormat::StreamJson {
            // How full the window is after this request: status lines show it
            // as a percentage, and `lazar jobs` reads it from child logs.
            let served = SERVED_MODEL.with(|slot| slot.borrow_mut().take());
            emit_event(json!({
                "type": "context",
                "input_tokens": usage_in.saturating_add(cache_read),
                "output_tokens": usage_out,
                "window": window.tokens,
                "model": &model,
                "served_model": served.unwrap_or_else(|| model.clone()),
            }));
        }
        if stop_reason_str == "model_context_window_exceeded" && !overflow_retried {
            overflow_retried = true;
            let current = core::compact::bytes(&messages);
            let plan = CompactPlan { ceiling: current / 2, target: current / 3 };
            if let Err(msg) = compact_messages(&mut messages, plan, &mut summarise, format) {
                return Err(abort_invoke(format, &session_guard, "context_limit", msg));
            }
            continue;
        }
        overflow_retried = false;

        messages.push(json!({"role": "assistant", "content": content.clone()}));
        append_stream(json!({"kind": "assistant", "content": content.clone()}));
        session_write_failed(format, &session_guard)?;

        let stop_reason = stop_reason_str.as_str();

        if is_terminal_stop_reason(stop_reason) && verify_enabled() {
            // Grounded verification: claims in the answer are checked against
            // the real filesystem before the turn is allowed to end.
            if let Some(parsed) = core::verify::extract_block(&final_answer_text(&messages)) {
                emit_phase(format, "verifying");
                let outcomes = match parsed {
                    Ok(lines) => core::verify::check_all(&lines, access::current().tool_cwd()),
                    Err(detail) => vec![core::verify::Outcome {
                        claim: "[VERIFY]".into(),
                        ok: false,
                        detail,
                    }],
                };
                let passed = outcomes.iter().all(|o| o.ok);
                verify_attempts += 1;
                let retry = !passed && verify_attempts <= core::verify::MAX_CORRECTIONS;
                let event = json!({
                    "status": if passed { "passed" } else { "failed" },
                    "attempt": verify_attempts,
                    "final": !retry,
                    "tools_run": tool_calls,
                    "claims": outcomes.iter().map(core::verify::Outcome::to_json).collect::<Vec<_>>(),
                });
                let mut logged = event.clone();
                logged["kind"] = json!("verify");
                append_stream(logged);
                if format == OutputFormat::StreamJson {
                    let mut emitted = event;
                    emitted["type"] = json!("verify");
                    emit_event(emitted);
                }
                if retry {
                    let correction = core::verify::correction_prompt(&outcomes);
                    append_stream(json!({"kind": "verify_correction", "content": &correction}));
                    session_write_failed(format, &session_guard)?;
                    messages.push(json!({"role": "user", "content": correction}));
                    continue;
                }
                if !passed {
                    let msg = core::verify::failure_message(&outcomes);
                    return Err(abort_invoke(format, &session_guard, "grounding_failed", msg));
                }
                verify_summary = json!({"status": "passed", "claims": outcomes.len()});
            }
        }

        if is_terminal_stop_reason(stop_reason) {
            let final_text = text::extract_text(&content);
            let answer = core::answer::enabled().then(|| {
                core::answer::build(
                    &final_answer_text(&messages),
                    access::current().tool_cwd(),
                    verify_summary.clone(),
                )
            });
            if let Some(answer) = &answer {
                let mut logged = answer.clone();
                logged["kind"] = json!("answer");
                append_stream(logged);
                if format == OutputFormat::StreamJson {
                    let mut emitted = answer.clone();
                    emitted["type"] = json!("answer");
                    emit_event(emitted);
                }
            }
            let duration_ms = started.elapsed().as_millis();
            fire_agent_stop(depth, stop_reason, duration_ms);

            append_stream(json!({
                "kind": "invoke_end",
                "stop_reason": stop_reason,
                "input_tokens": total_usage_in,
                "output_tokens": total_usage_out,
                "cache_read_input_tokens": total_cache_read,
            }));
            session_write_failed(format, &session_guard)?;

            if format == OutputFormat::StreamJson {
                emit_event(json!({
                    "type": "invoke_end",
                    "stop_reason": stop_reason,
                    "duration_ms": duration_ms,
                    "input_tokens": total_usage_in,
                    "output_tokens": total_usage_out,
                }));
            } else if format == OutputFormat::Json {
                let mut result = json!({
                    "type": "result",
                    "stop_reason": stop_reason,
                    "model": &model,
                    "result": final_text,
                    "content": content,
                    "duration_ms": duration_ms,
                    "input_tokens": total_usage_in,
                    "output_tokens": total_usage_out,
                });
                if let Some(answer) = answer {
                    result["answer"] = answer;
                }
                println!("{result}");
            }

            if verbose {
                eprintln!("[lazar] invoke_end stop_reason={stop_reason} duration={duration_ms}ms");
            }
            session_guard.mark_ok();
            return Ok(AgentOutcome {
                stop_reason: stop_reason.to_string(),
            });
        }

        // An answer cut by the output budget is continued, not failed. Tool
        // calls in a cut turn run as usual; only a pure-text cut continues.
        let has_tool_use = content
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_use"));
        if stop_reason == "max_tokens" && !has_tool_use {
            append_stream(json!({"kind": "continuation", "reason": "max_tokens"}));
            session_write_failed(format, &session_guard)?;
            if format == OutputFormat::StreamJson {
                emit_event(json!({"type": "continuation", "reason": "max_tokens"}));
            }
            messages.push(json!({"role": "user", "content": core::compact::CONTINUE_PROMPT}));
            continue;
        }

        let mut results = vec![];
        let mut had_any_valid_call = false;
        for b in content.as_array().unwrap_or(&vec![]) {
            if acp::is_session_cancelled() {
                return Err(abort_invoke(format, &session_guard, "cancelled", "cancelled".into()));
            }
            // Accept both "bash" (current name, primes the model with strong
            // shell-execution priors) and "execute" (legacy name; in-flight
            // sessions or external callers may still emit this).
            let is_bash_call = b["type"] == "tool_use"
                && (b["name"] == "bash" || b["name"] == "execute");
            if !is_bash_call {
                continue;
            }
            tool_calls = tool_calls.saturating_add(1);
            if tool_calls > limits.max_tool_calls {
                let msg = format!(
                    "aborted after reaching LAZAR_MAX_TOOL_CALLS={}",
                    limits.max_tool_calls
                );
                return Err(abort_invoke(format, &session_guard, "aborted", msg));
            }

            let outcome = execute_tool_use(b, depth, verbose, format);
            if outcome.valid_command {
                had_any_valid_call = true;
            }
            results.push(outcome.result_event);
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
                return Err(abort_invoke(format, &session_guard, "aborted", msg));
            }
        } else if had_any_valid_call {
            consecutive_empty_turns = 0;
        }

        if results.is_empty() {
            let msg = format!(
                "API stopped with {stop_reason:?} before a terminal response and without executable bash calls"
            );
            return Err(abort_invoke(format, &session_guard, "error", msg));
        }

        session_write_failed(format, &session_guard)?;
        messages.push(json!({"role": "user", "content": results}));
    }
}

/// Byte goals for one compaction, measured on the message list alone.
#[derive(Debug, Clone, Copy)]
struct CompactPlan {
    /// The messages must end at or below this.
    ceiling: usize,
    /// A summary aims here, leaving room for the rounds that follow.
    target: usize,
}

/// Shrink `messages` below `plan.ceiling`, cheapest stage first (see
/// `core::compact`). Logs and emits one `compaction` event. Errs only when
/// even per-item truncation cannot fit, which needs a window smaller than a
/// single tool round.
fn compact_messages(
    messages: &mut Vec<Value>,
    plan: CompactPlan,
    summarise: &mut dyn FnMut(&str) -> Option<String>,
    format: OutputFormat,
) -> Result<(), String> {
    use core::compact;
    emit_phase(format, "compacting");
    let before = compact::bytes(messages);
    let fits = |m: &Vec<Value>| compact::bytes(m) <= plan.ceiling;
    let mut stage = "elide";
    compact::elide_old_tool_results(messages, compact::KEEP_RECENT_TOOL_ROUNDS, compact::ELIDED_ITEM_BYTES);
    if !fits(messages) {
        if let Some(cut) = compact::summary_cut(messages, plan.target / 2) {
            stage = "summary";
            let transcript = compact::transcript(&messages[..cut], plan.target / 2);
            let summary = summarise(&transcript)
                .unwrap_or_else(|| compact::digest(&messages[..cut], plan.target / 4));
            compact::replace_head(messages, cut, &compact::elide(&summary, plan.target / 4));
        }
    }
    let mut item = (plan.ceiling / 4).min(core::context::MAX_CONTEXT_ITEM_BYTES);
    while !fits(messages) && item >= 256 {
        stage = "truncate";
        compact::truncate_items(messages, item);
        item /= 2;
    }
    let after = compact::bytes(messages);
    let event = json!({
        "stage": stage,
        "before_bytes": before,
        "after_bytes": after,
        "messages": messages.len(),
    });
    let mut logged = event.clone();
    logged["kind"] = json!("compaction");
    append_stream(logged);
    if format == OutputFormat::StreamJson {
        let mut emitted = event;
        emitted["type"] = json!("compaction");
        emit_event(emitted);
    }
    if fits(messages) {
        Ok(())
    } else {
        Err(format!(
            "context could not be compacted below {} bytes (at {after}); the model's context window is too small for this turn",
            plan.ceiling
        ))
    }
}

/// `lazar verify`: the kernel's claim checker for scripts and sub-agent
/// wrappers, so every caller shares one claim language.
fn run_verify(lint: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
    let lines: Vec<String> = match core::verify::extract_block(&input) {
        Some(Ok(lines)) => lines,
        Some(Err(detail)) => {
            println!("{}", json!({"passed": false, "error": detail, "claims": []}));
            std::process::exit(1);
        }
        None => input
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    };
    let cwd = env::current_dir()?;
    let outcomes: Vec<core::verify::Outcome> = if lint {
        lines
            .iter()
            .map(|line| match core::verify::parse(line, &cwd) {
                Ok(_) => core::verify::Outcome { claim: line.clone(), ok: true, detail: "valid".into() },
                Err(detail) => core::verify::Outcome { claim: line.clone(), ok: false, detail },
            })
            .collect()
    } else {
        core::verify::check_all(&lines, &cwd)
    };
    let passed = !outcomes.is_empty() && outcomes.iter().all(|o| o.ok);
    let claims: Vec<Value> = outcomes.iter().map(core::verify::Outcome::to_json).collect();
    println!("{}", json!({"passed": passed, "claims": claims}));
    if passed {
        Ok(())
    } else {
        std::process::exit(1)
    }
}

/// Announce what the turn is doing that a client cannot see from the stream:
/// waiting on the model, compacting, verifying. Text streaming and tool calls
/// already arrive as their own events.
fn emit_phase(format: OutputFormat, phase: &str) {
    if format == OutputFormat::StreamJson {
        emit_event(json!({"type": "phase", "phase": phase}));
    }
}

/// Verification is on unless the operator turns it off (`LAZAR_VERIFY=0`).
fn verify_enabled() -> bool {
    !matches!(
        env::var("LAZAR_VERIFY").ok().as_deref().map(str::trim),
        Some("0" | "off" | "false" | "no")
    )
}

/// The text of the answer that just ended: the last assistant message plus
/// any earlier parts it continued after an output-limit cut.
fn final_answer_text(messages: &[Value]) -> String {
    let mut parts = Vec::new();
    let mut index = messages.len();
    while index > 0 {
        index -= 1;
        if messages[index]["role"] != "assistant" {
            break;
        }
        parts.push(text::extract_text(&messages[index]["content"]));
        let continued = index > 0
            && messages[index - 1]["content"].as_str() == Some(core::compact::CONTINUE_PROMPT);
        if !continued {
            break;
        }
        index -= 1;
    }
    parts.reverse();
    parts.join("")
}

/// True when a provider error means the request was too large for the model.
fn is_context_overflow(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "prompt is too long",
        "context length",
        "context_length",
        "context window",
        "maximum context",
        "too many tokens",
        "token limit",
        "request too large",
        "413 payload too large",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

/// Heartbeat path: fire all hooks under hooks/tick.d/ and exit.
/// No model call, no agent loop. Wire this to launchd/cron for scheduled
/// background work like memory consolidation, summary enrichment, etc.
fn run_tick(verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    let started = std::time::Instant::now();
    append_stream(json!({"kind": "tick_start"}));
    if verbose {
        eprintln!("[lazar] tick_start");
    }

    let results = hooks::fire(
        HookEvent::Tick,
        json!({
            "ts_ms": now_millis(),
        }),
    );

    let duration_ms = started.elapsed().as_millis();
    append_stream(json!({
        "kind": "tick_end",
        "hooks_fired": results.len(),
        "duration_ms": duration_ms,
    }));
    if verbose {
        eprintln!(
            "[lazar] tick_end hooks_fired={} duration={duration_ms}ms",
            results.len()
        );
        for result in &results {
            eprintln!(
                "[lazar] tick_hook script={} exit={} duration={}ms timed_out={}",
                result.script.display(),
                result.exit_code,
                result.duration_ms,
                result.timed_out
            );
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // A parent shell must not silently inject a session path into an
    // invocation that did not request --session or --resume.
    env::remove_var("LAZAR_SESSION_ID");

    match args.command {
        Some(Command::Acp(acp_args)) => return acp::run(acp_args),
        Some(Command::Providers { json }) => return print_providers(json),
        Some(Command::Verify { lint }) => return run_verify(lint),
        Some(Command::Jobs { show, since }) => {
            let recent = since.map_or(jobs::RECENT_MS, |secs| u128::from(secs) * 1000);
            let dir = user_dir("workspace").join("subagent").join("jobs");
            match show {
                Some(id) => match jobs::detail(&dir, &id, now_millis()) {
                    Some(detail) => println!("{detail}"),
                    None => return Err(format!("no job with id {id}").into()),
                },
                None => println!("{}", Value::Array(jobs::list(&dir, now_millis(), recent))),
            }
            return Ok(());
        }
        Some(Command::Skills { json }) => {
            let skills = skill::list(&user_dir("skills"));
            if json {
                let rows: Vec<Value> = skills
                    .iter()
                    .map(|s| json!({"name": s.name, "description": s.description}))
                    .collect();
                println!("{}", Value::Array(rows));
            } else {
                for s in &skills {
                    println!("{:<28} {}", s.name, s.description);
                }
            }
            return Ok(());
        }
        None => {}
    }
    if let Some(provider) = args.provider.as_deref() {
        env::set_var("LAZAR_PROVIDER", provider);
    }
    if let Some(format) = args.answer_format.as_deref() {
        env::set_var("LAZAR_ANSWER_FORMAT", format);
    }
    if let Some(model) = args.model.as_deref() {
        env::set_var("LAZAR_MODEL", model);
    }

    if args.reset_all {
        return reset_all(args.yes);
    }

    prepare_user_layout();
    if args.sandbox {
        env::set_var("LAZAR_SANDBOX", "1");
    }
    if args.no_sandbox {
        env::set_var("LAZAR_NO_SANDBOX", "1");
    }
    let unsandboxed = no_sandbox_active();
    let (inherited_cwd, inherited_dirs) = access::inherited();
    let requested_cwd = args.cwd.as_deref().or(inherited_cwd.as_deref());
    let requested_dirs = if args.add_dirs.is_empty() {
        inherited_dirs.as_slice()
    } else {
        args.add_dirs.as_slice()
    };
    let access_policy = access::AccessPolicy::new(
        unsandboxed,
        requested_cwd,
        requested_dirs,
        &lazar_home(),
    )?;
    access::init(access_policy)?;

    // Make sure hooks/ exists for any path that fires hooks. First-run
    // installs that predate this kernel won't have the directory yet.
    ensure_hooks_seeded();

    // Operator-controlled sandbox bypass. Either the CLI flag or env var
    // sets the env var that run_bash/run_hook read on every spawn. The
    // agent itself cannot flip this on — env at spawn time is inherited
    // from this main process, and the agent is a tool subprocess that
    // can't mutate the parent's env.
    if unsandboxed {
        env::set_var("LAZAR_NO_SANDBOX", "1");
    } else {
        env::set_var("LAZAR_SANDBOX", "1");
    }
    append_stream(json!({
        "kind": "sandbox_mode",
        "enabled": !unsandboxed,
    }));
    append_stream(json!({
        "kind": "access_policy",
        "cwd": access::current().tool_cwd(),
        "allowed_write_dirs": access::current().allowed_write_dirs(),
        "unsandboxed": access::current().unsandboxed(),
    }));

    if args.tick {
        return run_tick(args.verbose);
    }

    let prompt = args
        .prompt
        .ok_or("must pass -p <prompt>, --tick, or --reset-all (see --help)")?;

    // Validate and export session id (if any) so append_stream and run_agent
    // can both pick it up via env. Hooks inherit env so they see it too.
    if let Some(ref id) = args.session {
        if let Err(msg) = session::validate_session_id(id) {
            return Err(format!("invalid --session id: {msg}").into());
        }
        env::set_var("LAZAR_SESSION_ID", id);
    } else if args.resume {
        match session::find_newest_session() {
            Some(id) => {
                if args.verbose {
                    eprintln!("[lazar] --resume picked session: {id}");
                }
                env::set_var("LAZAR_SESSION_ID", id);
            }
            None => {
                if args.verbose {
                    eprintln!(
                        "[lazar] --resume: no session logs found, starting fresh"
                    );
                }
                // Don't set LAZAR_SESSION_ID — agent runs as a normal
                // cold-start invocation. No error: --resume is a hint,
                // not a hard requirement.
            }
        }
    }

    run_agent(
        &prompt,
        args.output_format,
        args.verbose,
        args.model,
        AgentLifecycle::default(),
    )
    .map(|_| ())
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    /// Serializes every test that mutates process-global environment
    /// (LAZAR_HOME, provider vars, ...). `cargo test` runs tests on several
    /// threads; without this, a test that points LAZAR_HOME at a temp dir
    /// can make another test write into it — or into the operator's real
    /// ~/lazar/logs/stream.jsonl.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Saves the named variables on creation and restores them on drop, so a
    /// test can mutate the environment freely (under `env_lock`).
    pub(crate) struct EnvSnapshot {
        saved: Vec<(String, Option<std::ffi::OsString>)>,
    }

    impl EnvSnapshot {
        pub(crate) fn capture(keys: &[&str]) -> Self {
            Self {
                saved: keys
                    .iter()
                    .map(|k| (k.to_string(), std::env::var_os(k)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                match value {
                    Some(value) => std::env::set_var(&key, value),
                    None => std::env::remove_var(&key),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    // ─── Loopback fake of the Anthropic Messages API ──────────────────
    // Serves one canned SSE body per accepted connection and records every
    // request body, so a test can drive the real agent loop end to end and
    // assert what the kernel actually sent back (tool results, corrections).

    struct FakeApi {
        base_url: String,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl FakeApi {
        fn serve(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&requests);
            thread::spawn(move || {
                for body in responses {
                    let Ok((mut stream, _)) = listener.accept() else { return };
                    seen.lock().unwrap().push(read_http_request(&mut stream));
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(reply.as_bytes());
                    let _ = stream.flush();
                }
            });
            Self { base_url, requests }
        }

        fn requests(&self) -> Vec<Value> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|body| serde_json::from_str(body).expect("kernel sends JSON bodies"))
                .collect()
        }
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut chunk).expect("read request headers");
            assert!(n > 0, "client closed before sending headers");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
        let content_length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        while buf.len() < header_end + content_length {
            let n = stream.read(&mut chunk).expect("read request body");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        String::from_utf8_lossy(&buf[header_end..]).into_owned()
    }

    fn sse(events: &[Value]) -> String {
        events.iter().map(|e| format!("data: {e}\n\n")).collect()
    }

    /// One model turn that calls the bash tool with `cmd`.
    fn sse_tool_use(cmd: &str) -> String {
        sse(&[
            json!({"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"bash","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":json!({"command":cmd}).to_string()}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
            json!({"type":"message_stop"}),
        ])
    }

    /// One terminal model turn with plain text.
    fn sse_end_turn(text: &str) -> String {
        sse(&[
            json!({"type":"message_start","message":{"usage":{"input_tokens":20,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
            json!({"type":"message_stop"}),
        ])
    }

    const AGENT_TEST_VARS: &[&str] = &[
        "LAZAR_HOME",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "MINIMAX_API_KEY",
        "LAZAR_MODEL",
        "LAZAR_PROVIDER",
        "LAZAR_SESSION_ID",
        "LAZAR_NO_SANDBOX",
        "LAZAR_MAX_TURNS",
        "LAZAR_MAX_TOOL_CALLS",
        "LAZAR_STREAM_IDLE_SECS",
        "LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY",
        "LAZAR_TOOL_INHERIT_PROVIDER_KEY",
    ];

    /// Scoped environment for agent-loop tests: a temp LAZAR_HOME (so the
    /// loop never touches the operator's real ~/lazar), the loopback
    /// provider, no session, kernel defaults for every limit. Holds the
    /// crate-wide env lock for its whole lifetime.
    struct AgentTestEnv {
        home: PathBuf,
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl AgentTestEnv {
        fn new(label: &str, api: &FakeApi) -> Self {
            let guard = crate::test_support::env_lock();
            let saved = AGENT_TEST_VARS
                .iter()
                .map(|key| (*key, env::var_os(key)))
                .collect();
            let home = env::temp_dir().join(format!(
                "lazar-agent-{label}-{}-{}",
                std::process::id(),
                now_millis()
            ));
            fs::create_dir_all(home.join("user/workspace")).expect("create temp home");
            for key in AGENT_TEST_VARS {
                env::remove_var(key);
            }
            env::set_var("LAZAR_HOME", &home);
            env::set_var("ANTHROPIC_API_KEY", "test-key");
            env::set_var("ANTHROPIC_BASE_URL", &api.base_url);
            Self {
                home,
                saved,
                _guard: guard,
            }
        }

        fn log_events(&self) -> Vec<Value> {
            fs::read_to_string(self.home.join("user/logs/stream.jsonl"))
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        }
    }

    impl Drop for AgentTestEnv {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
            let _ = fs::remove_dir_all(&self.home);
        }
    }

    #[test]
    fn agent_loop_executes_tool_calls_then_ends_on_end_turn() {
        let api = FakeApi::serve(vec![
            sse_tool_use("printf probe-ok > \"$LAZAR_WORKSPACE/probe.txt\""),
            sse_end_turn("Wrote the probe file."),
        ]);
        let test_env = AgentTestEnv::new("happy", &api);

        run_agent(
            "write the probe",
            OutputFormat::Json,
            false,
            None,
            AgentLifecycle::default(),
        )
        .expect("agent loop should complete");

        // The bash tool really ran, inside the temp workspace.
        let probe = test_env.home.join("user/workspace/probe.txt");
        assert_eq!(fs::read_to_string(&probe).expect("probe written"), "probe-ok");

        // The second request carried the tool result back to the model.
        let requests = api.requests();
        assert_eq!(requests.len(), 2, "one tool turn + one terminal turn");
        assert_eq!(requests[0]["messages"].as_array().map(Vec::len), Some(1));
        assert!(requests[0]["system"]
            .as_str()
            .is_some_and(|s| s.contains("You are lazar") && s.contains("ACCESS POLICY")));
        let messages = requests[1]["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 3, "user, assistant(tool_use), user(tool_result)");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        let result = &messages[2]["content"][0];
        assert_eq!(result["type"], "tool_result");
        assert_eq!(result["tool_use_id"], "toolu_1");
        assert!(result["content"].as_str().is_some_and(|c| c.ends_with("[exit 0]")));

        // The canonical log recorded the whole invocation in the temp home.
        let events = test_env.log_events();
        let kinds: Vec<&str> = events
            .iter()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        for expected in ["invoke_start", "user", "assistant", "tool_result", "invoke_end"] {
            assert!(kinds.contains(&expected), "missing {expected} in {kinds:?}");
        }
        let tool_result = events.iter().find(|e| e["kind"] == "tool_result").unwrap();
        assert_eq!(tool_result["exit_code"], 0);
        let invoke_end = events.iter().find(|e| e["kind"] == "invoke_end").unwrap();
        assert_eq!(invoke_end["stop_reason"], "end_turn");
        assert_eq!(invoke_end["input_tokens"], 30);
        assert_eq!(invoke_end["output_tokens"], 8);
    }

    #[test]
    fn agent_loop_trusts_end_turn_and_never_inspects_prose() {
        // The model narrates an action and stops. Per the Messages protocol
        // that is a finished turn. The kernel must return it to the operator
        // in ONE request — no auto-nudge, no forced tool_choice, no
        // "completion review" of the wording. Recovering from a model that
        // announces work instead of calling the tool is the operator's (or a
        // better model's) job, not a prose heuristic's.
        let api = FakeApi::serve(vec![sse_end_turn(
            "Sure. Let me check what's changed. First, git status. I'll run it now:",
        )]);
        let test_env = AgentTestEnv::new("trust-stop-reason", &api);

        run_agent("look around", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect("a narrated end_turn is a completed turn");

        let requests = api.requests();
        assert_eq!(requests.len(), 1, "exactly one model call; nothing re-prompted");
        assert!(requests[0].get("tool_choice").is_none());
        assert!(requests[0]["system"]
            .as_str()
            .unwrap()
            .contains("[VERIFY]"), "system prompt carries the VERIFY contract");

        let events = test_env.log_events();
        let kinds: Vec<&str> = events.iter().filter_map(|e| e["kind"].as_str()).collect();
        for forbidden in [
            "auto_nudge",
            "completion_review",
            "grounding_retry",
            "grounding_failure",
            "fabrication_check",
            "verify_report",
            // No [VERIFY] block means nothing was asserted: no check runs.
            "verify",
        ] {
            assert!(!kinds.contains(&forbidden), "{forbidden} must not exist: {kinds:?}");
        }
        assert!(events
            .iter()
            .any(|e| e["kind"] == "invoke_end" && e["stop_reason"] == "end_turn"));
    }

    #[test]
    fn session_log_write_failure_fails_the_turn() {
        let api = FakeApi::serve(vec![sse_end_turn("hi")]);
        let test_env = AgentTestEnv::new("session-write", &api);
        let id = "blocked";
        env::set_var("LAZAR_SESSION_ID", id);
        let sessions = test_env.home.join("user/logs/sessions");
        fs::create_dir_all(&sessions).unwrap();
        let blocked = sessions.join(format!("{id}.jsonl"));
        fs::write(&blocked, "").unwrap();
        let mut permissions = fs::metadata(&blocked).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&blocked, permissions).unwrap();

        let err = run_agent("hi", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect_err("a session that cannot be written must fail the turn");
        assert!(err.to_string().contains("session log"), "{err}");
        assert!(
            !test_env.log_events().iter().any(|event| {
                event["kind"] == "invoke_end" && event["stop_reason"] == "end_turn"
            }),
            "a failed session write must not record a successful invoke_end"
        );
        assert!(api.requests().is_empty(), "the provider is not called once the log write fails");
    }

    #[test]
    fn interrupted_tool_is_recorded_as_outcome_unknown_without_rerunning() {
        let api = FakeApi::serve(vec![]);
        let test_env = AgentTestEnv::new("repair", &api);
        let id = "repair";
        env::set_var("LAZAR_SESSION_ID", id);
        let path = test_env.home.join("user/logs/sessions").join(format!("{id}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let events = [
            json!({"kind":"user","content":"go"}),
            json!({"kind":"assistant","content":[{"type":"tool_use","id":"toolu_open","name":"bash","input":{"command":"printf ran > \"$LAZAR_WORKSPACE/ran.txt\""}}]}),
        ];
        fs::write(&path, events.iter().map(|event| format!("{event}\n")).collect::<String>()).unwrap();

        let messages = session::load_messages_checked(id, 32_000).expect("repair load");
        let repaired = messages
            .iter()
            .find_map(|message| {
                message["content"].as_array().and_then(|blocks| {
                    blocks.iter().find_map(|block| {
                        (block["type"] == "tool_result")
                            .then(|| block["content"].as_str().unwrap_or("").to_string())
                    })
                })
            })
            .unwrap_or_default();
        assert!(repaired.contains("may already have happened"), "{messages:#?}");
        let persisted = fs::read_to_string(&path).unwrap();
        assert!(persisted.contains("may already have happened"));
        assert!(!test_env.home.join("user/workspace/ran.txt").exists());
        assert!(api.requests().is_empty());
    }

    #[test]
    fn cut_stream_keeps_partial_assistant_and_does_not_retry() {
        let partial = "partial answer";
        let body = format!(
            "data: {}\n\ndata: {}\n\n",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":partial}}),
        );
        let api = FakeApi::serve(vec![body, sse_end_turn("should not be requested")]);
        let test_env = AgentTestEnv::new("cut-stream", &api);
        env::set_var("LAZAR_SESSION_ID", "cut");

        let err = run_agent("continue", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect_err("a cut stream is an error");
        assert!(err.to_string().contains("message_stop"), "{err}");
        assert_eq!(api.requests().len(), 1, "a cut stream is not re-issued");
        let session = fs::read_to_string(test_env.home.join("user/logs/sessions/cut.jsonl")).unwrap();
        assert!(session.contains(partial), "{session}");
        assert!(session.contains("\"kind\":\"assistant\""));
    }

    #[test]
    fn idle_provider_body_fails_before_the_request_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("local addr");
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let _ = read_http_request(&mut stream);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 100000\r\n\r\n",
            );
            let _ = stream.flush();
            thread::sleep(Duration::from_secs(30));
        });
        let api = FakeApi::serve(vec![]);
        let _test_env = AgentTestEnv::new("idle", &api);
        env::set_var("ANTHROPIC_BASE_URL", format!("http://{address}"));
        env::set_var("LAZAR_STREAM_IDLE_SECS", "1");
        let started = Instant::now();
        let err = run_agent("hi", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect_err("a silent body must time out");
        assert!(started.elapsed() < Duration::from_secs(20), "idle guard took {:?}", started.elapsed());
        assert!(err.to_string().contains("idle"), "{err}");
        assert!(err.to_string().contains("response body"), "{err}");
    }

    /// One model turn of plain text ending with `stop_reason`.
    fn sse_text_stop(text: &str, stop_reason: &str) -> String {
        sse(&[
            json!({"type":"message_start","message":{"usage":{"input_tokens":20,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":stop_reason},"usage":{"output_tokens":3}}),
            json!({"type":"message_stop"}),
        ])
    }

    #[test]
    fn verified_claims_pass_in_one_request() {
        let api = FakeApi::serve(vec![
            sse_tool_use("printf hello > \"$LAZAR_WORKSPACE/proof.txt\""),
            sse_end_turn("Wrote it.\n[VERIFY]\nexists=$LAZAR_HOME/user/workspace/proof.txt\ncontains=$LAZAR_HOME/user/workspace/proof.txt:hello\n[/VERIFY]"),
        ]);
        let test_env = AgentTestEnv::new("verify-pass", &api);
        let outcome = run_agent("write proof", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect("true claims pass");
        assert_eq!(outcome.stop_reason, "end_turn");
        assert_eq!(api.requests().len(), 2, "no correction turn");
        let verify = test_env.log_events().into_iter().find(|e| e["kind"] == "verify").unwrap();
        assert_eq!(verify["status"], "passed");
        assert_eq!(verify["claims"].as_array().unwrap().len(), 2);
        assert_eq!(verify["tools_run"], 1);
    }

    #[test]
    fn a_false_claim_gets_one_correction_turn() {
        let api = FakeApi::serve(vec![
            sse_end_turn("Done.\n[VERIFY]\nexists=$LAZAR_HOME/user/workspace/made.txt\n[/VERIFY]"),
            sse_tool_use("touch \"$LAZAR_WORKSPACE/made.txt\""),
            sse_end_turn("Now it exists.\n[VERIFY]\nexists=$LAZAR_HOME/user/workspace/made.txt\n[/VERIFY]"),
        ]);
        let test_env = AgentTestEnv::new("verify-correct", &api);
        let outcome = run_agent("make it", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect("the corrected answer passes");
        assert_eq!(outcome.stop_reason, "end_turn");
        let requests = api.requests();
        assert_eq!(requests.len(), 3);
        let correction = requests[1]["messages"][2]["content"].as_str().unwrap();
        assert!(correction.starts_with("[kernel verification]"), "{correction}");
        assert!(correction.contains("made.txt") && correction.contains("does not exist"), "{correction}");
        let statuses: Vec<Value> = test_env
            .log_events()
            .into_iter()
            .filter(|e| e["kind"] == "verify")
            .map(|e| e["status"].clone())
            .collect();
        assert_eq!(statuses, vec![json!("failed"), json!("passed")]);
    }

    #[test]
    fn a_claim_that_fails_twice_ends_as_grounding_failed() {
        let lie = "Deleted.\n[VERIFY]\nnot_exists=/\n[/VERIFY]";
        let api = FakeApi::serve(vec![sse_end_turn(lie), sse_end_turn(lie)]);
        let test_env = AgentTestEnv::new("verify-fail", &api);
        let err = run_agent("delete root", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect_err("a repeated false claim fails the turn");
        assert!(err.to_string().contains("grounding failed"), "{err}");
        assert_eq!(api.requests().len(), 2, "exactly one correction turn");
        assert!(test_env
            .log_events()
            .iter()
            .any(|e| e["kind"] == "invoke_end" && e["stop_reason"] == "grounding_failed"));
    }

    #[test]
    fn structured_answers_carry_reply_speech_sources_and_verification() {
        let api = FakeApi::serve(vec![
            sse_tool_use("printf hi > \"$LAZAR_WORKSPACE/a.txt\""),
            sse_end_turn("Wrote a.txt.\n[SPEAK]\nI wrote the file.\n[/SPEAK]\n[SOURCES]\n$LAZAR_HOME/user/workspace/a.txt\n[/SOURCES]\n[VERIFY]\ncontains=$LAZAR_HOME/user/workspace/a.txt:hi\n[/VERIFY]"),
        ]);
        let test_env = AgentTestEnv::new("answer", &api);
        let _format = crate::test_support::EnvSnapshot::capture(&["LAZAR_ANSWER_FORMAT"]);
        env::set_var("LAZAR_ANSWER_FORMAT", "structured");
        run_agent("write a", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect("structured turn");
        assert!(api.requests()[0]["system"].as_str().unwrap().contains("ANSWER FORMAT"));
        let answer = test_env.log_events().into_iter().find(|e| e["kind"] == "answer").unwrap();
        assert_eq!(answer["reply"], "Wrote a.txt.");
        assert_eq!(answer["speech"], "I wrote the file.");
        assert_eq!(answer["sources"][0]["exists"], true);
        assert_eq!(answer["verify"], json!({"status": "passed", "claims": 1}));

        // Raw mode (the default) adds no contract and no record.
        env::remove_var("LAZAR_ANSWER_FORMAT");
        let raw = FakeApi::serve(vec![sse_end_turn("hello")]);
        env::set_var("ANTHROPIC_BASE_URL", &raw.base_url);
        run_agent("hi", OutputFormat::Json, false, None, AgentLifecycle::default()).unwrap();
        assert!(!raw.requests()[0]["system"].as_str().unwrap().contains("ANSWER FORMAT"));
        assert_eq!(test_env.log_events().iter().filter(|e| e["kind"] == "answer").count(), 1);
    }

    #[test]
    fn output_limit_cut_is_continued_not_failed() {
        let api = FakeApi::serve(vec![
            sse_text_stop("part one", "max_tokens"),
            sse_end_turn(" part two"),
        ]);
        let _test_env = AgentTestEnv::new("continue", &api);
        let outcome = run_agent("write a lot", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect("a cut reply continues");
        assert_eq!(outcome.stop_reason, "end_turn");
        let requests = api.requests();
        assert_eq!(requests.len(), 2);
        let messages = requests[1]["messages"].as_array().unwrap();
        assert_eq!(messages[1]["content"][0]["text"], "part one");
        assert_eq!(messages[2]["content"], core::compact::CONTINUE_PROMPT);
    }

    #[test]
    fn provider_overflow_compacts_and_retries_once() {
        let api = FakeApi::serve(vec![
            sse_tool_use("head -c 40000 /dev/zero | tr '\\0' y"),
            sse_text_stop("", "model_context_window_exceeded"),
            sse_end_turn("summary of the work"),
            sse_end_turn("done"),
        ]);
        let _test_env = AgentTestEnv::new("overflow", &api);
        let outcome = run_agent("read the big file", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect("overflow is recovered by compaction");
        assert_eq!(outcome.stop_reason, "end_turn");
        let requests = api.requests();
        assert_eq!(requests.len(), 4, "tool, overflow, summary, retry");
        assert_eq!(requests[2]["system"], core::compact::SUMMARY_SYSTEM_PROMPT);
        assert!(requests[2].get("tools").is_none(), "the summariser gets no tools");
        let sent = |i: usize| requests[i]["messages"].to_string().len();
        assert!(sent(3) < sent(1) / 2, "retry {} vs overflow {}", sent(3), sent(1));
        let first = requests[3]["messages"][0]["content"].as_str().unwrap();
        assert!(first.contains("summary of the work") && first.contains("read the big file"), "{first}");
    }

    #[test]
    fn staged_compaction_summarises_then_fits() {
        let mut messages = vec![json!({"role":"user","content":"original task"})];
        for i in 0..10 {
            messages.push(json!({"role":"assistant","content":[{"type":"tool_use","id":format!("t{i}"),"name":"bash","input":{"command":"cat f"}}]}));
            messages.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":format!("t{i}"),"content":"z".repeat(5_000)}]}));
        }
        let plan = CompactPlan { ceiling: 14_000, target: 10_000 };
        let mut calls = 0;
        let mut summarise = |_: &str| {
            calls += 1;
            Some("model summary".to_string())
        };
        compact_messages(&mut messages, plan, &mut summarise, OutputFormat::Json).unwrap();
        assert_eq!(calls, 1);
        assert!(core::compact::bytes(&messages) <= 14_000);
        assert!(messages[0]["content"].as_str().unwrap().contains("model summary"));
        assert_eq!(messages[1]["role"], "assistant");

        // Summariser down: the mechanical digest stands in.
        let mut again = vec![json!({"role":"user","content":"task two"})];
        for i in 0..10 {
            again.push(json!({"role":"assistant","content":[{"type":"text","text":"x".repeat(3_000)},{"type":"tool_use","id":format!("u{i}"),"name":"bash","input":{"command":"ls"}}]}));
            again.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":format!("u{i}"),"content":"ok"}]}));
        }
        compact_messages(&mut again, plan, &mut |_: &str| None, OutputFormat::Json).unwrap();
        assert!(again[0]["content"].as_str().unwrap().contains("Mechanical digest"));
        assert!(core::compact::bytes(&again) <= 14_000);
    }

    #[test]
    fn nested_commands_inherit_turn_budgets() {
        let api = FakeApi::serve(vec![
            sse_tool_use("printf '%s %s' \"$LAZAR_MAX_TURNS\" \"$LAZAR_MAX_TOOL_CALLS\" > \"$LAZAR_WORKSPACE/budgets.txt\""),
            sse_end_turn("ok"),
        ]);
        let test_env = AgentTestEnv::new("budgets", &api);
        env::set_var("LAZAR_MAX_TURNS", "3");
        env::set_var("LAZAR_MAX_TOOL_CALLS", "4");
        run_agent("show budgets", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect("budget probe");
        assert_eq!(
            fs::read_to_string(test_env.home.join("user/workspace/budgets.txt")).unwrap(),
            "3 4"
        );
    }

    #[test]
    fn raw_lazar_prompt_is_refused_by_the_dispatcher() {
        assert!(command_invokes_raw_lazar("lazar -p hi"));
        // Credential hunting is refused; ordinary env use is not.
        for hunt in [
            "security find-generic-password -a FREELLM_API_KEY -s lazar -w",
            "cat /proc/$$/environ | tr '\\0' '\\n'",
            "env | grep -i API_KEY",
            "printenv | grep TOKEN",
        ] {
            assert!(command_hunts_credentials(hunt), "{hunt}");
        }
        for fine in [
            "env | grep PATH",
            "echo $HOME",
            "grep -r key_env src/",
            "export PATH=/x:$PATH; grep -r token src/",
            "cargo test",
        ] {
            assert!(!command_hunts_credentials(fine), "{fine}");
        }
        assert!(command_invokes_raw_lazar("echo done; lazar --prompt=hi"));
        assert!(!command_invokes_raw_lazar("bash $LAZAR_HOME/bin/lazar-spawn t1 prompt.txt"));

        let api = FakeApi::serve(vec![
            sse_tool_use("lazar -p 'printf spawned > \"$LAZAR_WORKSPACE/spawned.txt\"'"),
            sse_end_turn("stopped"),
        ]);
        let test_env = AgentTestEnv::new("no-self-spawn", &api);
        run_agent("delegate", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect("veto is a normal tool result");
        assert!(!test_env.home.join("user/workspace/spawned.txt").exists());
        let result = api.requests()[1]["messages"][2]["content"][0]["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(result.contains("lazar -p"), "{result}");
    }

    #[test]
    fn hook_context_does_not_change_the_stable_system_prefix() {
        let _guard = crate::test_support::env_lock();
        let first = build_system_prompt(0, "alpha hook", DEFAULT_MODEL, "https://api.anthropic.com/v1/messages", Some("anthropic")).unwrap();
        let second = build_system_prompt(0, "beta hook", DEFAULT_MODEL, "https://api.anthropic.com/v1/messages", Some("anthropic")).unwrap();
        let prefix = |prompt: &str| {
            prompt
                .split("\n\nRUNTIME CONTEXT")
                .next()
                .unwrap_or("")
                .to_string()
        };
        assert_eq!(prefix(&first), prefix(&second));
        assert!(first.contains("alpha hook"));
        assert!(second.contains("beta hook"));
        assert_ne!(first, second);
    }

    #[test]
    fn skill_directive_is_resolved_by_the_kernel_not_the_model() {
        let api = FakeApi::serve(vec![sse_end_turn("Status: all good.")]);
        let test_env = AgentTestEnv::new("skill", &api);
        let skill_dir = test_env.home.join("user/skills/lazar-status");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "# lazar-status\n\nRun `bash $LAZAR_HOME/workspace/lazar-status.sh` and report the output.\n",
        )
        .unwrap();

        run_agent(
            "[skill:lazar-status] Use this skill now.",
            OutputFormat::Json,
            false,
            None,
            AgentLifecycle::default(),
        )
        .expect("skill turn completes");

        // The model got the SKILL.md body inline; the bare directive never
        // reached it, so there is nothing for it to "load".
        let requests = api.requests();
        assert_eq!(requests.len(), 1);
        let user = requests[0]["messages"][0]["content"].as_str().unwrap();
        assert!(user.contains("invoked the skill `lazar-status`"), "{user}");
        assert!(user.contains("workspace/lazar-status.sh"), "{user}");
        assert!(user.ends_with("Operator request: Use this skill now."), "{user}");
        assert!(!user.starts_with("[skill:"), "raw directive must not be the prompt");

        // The log keeps what the operator actually typed, plus the injection.
        let events = test_env.log_events();
        let typed = events.iter().find(|e| e["kind"] == "user").unwrap();
        assert_eq!(typed["content"], "[skill:lazar-status] Use this skill now.");
        let inject = events.iter().find(|e| e["kind"] == "skill_inject").expect("skill_inject logged");
        assert_eq!(inject["name"], "lazar-status");
        assert!(inject["bytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn unknown_skill_directive_is_an_error_before_any_model_call() {
        let api = FakeApi::serve(vec![sse_end_turn("never reached")]);
        let test_env = AgentTestEnv::new("skill-missing", &api);
        fs::create_dir_all(test_env.home.join("user/skills/memory")).unwrap();
        fs::write(test_env.home.join("user/skills/memory/SKILL.md"), "# memory\n").unwrap();

        let err = run_agent(
            "[skill:lazar-statsu] Use this skill now.",
            OutputFormat::Json,
            false,
            None,
            AgentLifecycle::default(),
        )
        .expect_err("a misspelled skill is the client's error");
        assert!(err.to_string().contains("unknown skill `lazar-statsu`"), "{err}");
        assert!(err.to_string().contains("memory"), "lists what is installed: {err}");
        assert!(api.requests().is_empty(), "no provider call was made");
        assert!(test_env
            .log_events()
            .iter()
            .any(|e| e["kind"] == "invoke_end" && e["stop_reason"] == "error"));
    }

    #[test]
    fn agent_loop_aborts_after_three_consecutive_empty_tool_calls() {
        let api = FakeApi::serve(vec![sse_tool_use(""), sse_tool_use("   "), sse_tool_use("")]);
        let test_env = AgentTestEnv::new("empty", &api);

        let err = run_agent("do nothing", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect_err("empty tool calls must not loop forever");
        assert!(
            err.to_string().contains("3 consecutive turns with only empty tool calls"),
            "unexpected error: {err}"
        );

        let requests = api.requests();
        assert_eq!(requests.len(), 3);
        let refused = &requests[1]["messages"][2]["content"][0];
        assert_eq!(refused["type"], "tool_result");
        assert!(refused["content"]
            .as_str()
            .is_some_and(|c| c.contains("empty or missing 'command'") && c.ends_with("[exit 1]")));
        assert!(test_env
            .log_events()
            .iter()
            .any(|e| e["kind"] == "invoke_end" && e["stop_reason"] == "aborted"));
    }

    #[test]
    fn agent_loop_honors_the_tool_call_budget() {
        let api = FakeApi::serve(vec![sse_tool_use("true"), sse_tool_use("true")]);
        let test_env = AgentTestEnv::new("budget", &api);
        env::set_var("LAZAR_MAX_TOOL_CALLS", "1");

        let err = run_agent("spend", OutputFormat::Json, false, None, AgentLifecycle::default())
            .expect_err("second tool call must exceed LAZAR_MAX_TOOL_CALLS=1");
        assert!(err.to_string().contains("LAZAR_MAX_TOOL_CALLS=1"), "unexpected error: {err}");
        assert_eq!(api.requests().len(), 2);
        assert_eq!(
            test_env.log_events().iter().filter(|e| e["kind"] == "tool_result").count(),
            1,
            "only the first call may run"
        );
    }

    #[test]
    fn selected_provider_is_authoritative_and_ignores_stale_env() {
        let _guard = crate::test_support::env_lock();
        let _snapshot = crate::test_support::EnvSnapshot::capture(&[
            "LAZAR_HOME",
            "LAZAR_PROVIDER",
            "LAZAR_DEPTH",
            "LAZAR_MODEL",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "MINIMAX_API_KEY",
            "TESTP_API_KEY",
        ]);
        let home = env::temp_dir().join(format!(
            "lazar-provider-resolve-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("providers.toml"),
            "[[providers]]\nid = \"testp\"\nname = \"Test\"\nbase_url = \"https://testp.example/\"\nkey_env = \"TESTP_API_KEY\"\ndefault_model = \"testp-default\"\n",
        )
        .unwrap();
        env::set_var("LAZAR_HOME", &home);
        env::set_var("LAZAR_PROVIDER", "testp");
        env::set_var("TESTP_API_KEY", "sk-testp");
        // Residue from an earlier launcher must not leak into a selected provider.
        env::set_var("ANTHROPIC_API_KEY", "sk-stale");
        env::set_var("ANTHROPIC_BASE_URL", "https://stale.example");
        env::remove_var("LAZAR_MODEL");
        env::remove_var("LAZAR_DEPTH");

        let cfg = resolve_provider(None).unwrap();
        assert_eq!(cfg.api_url, "https://testp.example/v1/messages");
        assert_eq!(cfg.api_key, "sk-testp");
        assert_eq!(cfg.model, "testp-default");
        assert_eq!(cfg.provider_id.as_deref(), Some("testp"));

        assert_eq!(resolve_provider(Some("cli-model".into())).unwrap().model, "cli-model");
        env::set_var("LAZAR_MODEL", "env-model");
        assert_eq!(resolve_provider(None).unwrap().model, "env-model");

        // Nested invocation without the key in env: no Keychain, clear error.
        env::remove_var("TESTP_API_KEY");
        env::set_var("LAZAR_DEPTH", "1");
        let err = resolve_provider(None).unwrap_err();
        assert!(err.contains("TESTP_API_KEY"), "{err}");

        env::set_var("LAZAR_PROVIDER", "nope");
        assert!(resolve_provider(None)
            .unwrap_err()
            .contains("unknown provider 'nope'"));

        // Legacy contract without a selected provider is unchanged.
        env::remove_var("LAZAR_PROVIDER");
        env::remove_var("LAZAR_MODEL");
        let legacy = resolve_provider(None).unwrap();
        assert_eq!(legacy.api_url, "https://stale.example/v1/messages");
        assert_eq!(legacy.api_key, "sk-stale");
        assert_eq!(legacy.model, DEFAULT_MODEL);
        assert_eq!(legacy.provider_id, None);

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn tool_credential_inheritance_follows_the_active_provider() {
        let _guard = crate::test_support::env_lock();
        let _snapshot = crate::test_support::EnvSnapshot::capture(&[
            "LAZAR_HOME",
            "LAZAR_PROVIDER",
            "ANTHROPIC_API_KEY",
            "MINIMAX_API_KEY",
            "TESTP_API_KEY",
        ]);
        let home = env::temp_dir().join(format!(
            "lazar-provider-inherit-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("providers.toml"),
            "[[providers]]\nid = \"testp\"\nname = \"Test\"\nbase_url = \"https://testp.example\"\nkey_env = \"TESTP_API_KEY\"\ndefault_model = \"testp-default\"\n",
        )
        .unwrap();
        env::set_var("LAZAR_HOME", &home);
        env::set_var("LAZAR_PROVIDER", "testp");
        env::set_var("TESTP_API_KEY", "selected-key");
        env::set_var("ANTHROPIC_API_KEY", "stale-key");

        assert_eq!(
            active_provider_credential(0),
            Some(("TESTP_API_KEY".into(), "selected-key".into()))
        );

        env::remove_var("LAZAR_PROVIDER");
        env::remove_var("ANTHROPIC_API_KEY");
        env::set_var("MINIMAX_API_KEY", "minimax-key");
        assert_eq!(
            active_provider_credential(0),
            Some(("MINIMAX_API_KEY".into(), "minimax-key".into()))
        );

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn cli_accepts_provider_flag_and_providers_subcommand() {
        let args = Args::try_parse_from(["lazar", "--provider", "kimi", "-p", "hi"]).unwrap();
        assert_eq!(args.provider.as_deref(), Some("kimi"));
        let args = Args::try_parse_from(["lazar", "providers", "--json"]).unwrap();
        assert!(matches!(args.command, Some(Command::Providers { json: true })));
    }

    #[test]
    fn cli_accepts_acp_subcommand() {
        let args = Args::try_parse_from(["lazar", "acp", "--verbose", "--model", "claude-sonnet-4-6"])
            .expect("acp subcommand should parse");
        match args.command {
            Some(Command::Acp(acp)) => {
                assert!(acp.verbose);
                assert_eq!(acp.model.as_deref(), Some("claude-sonnet-4-6"));
            }
            other => panic!("expected acp subcommand, got {other:?}"),
        }
    }

    #[test]
    fn cli_accepts_explicit_cwd_and_repeatable_add_dirs() {
        let args = Args::try_parse_from([
            "lazar",
            "--cwd",
            "/tmp",
            "--add-dir",
            "/tmp/one",
            "--add-dir",
            "/tmp/two",
            "-p",
            "test",
        ])
        .expect("scoped access flags should parse");

        assert_eq!(args.cwd.as_deref(), Some(Path::new("/tmp")));
        assert_eq!(
            args.add_dirs,
            [PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")]
        );
    }

    #[test]
    fn reset_removes_saved_access_approvals() {
        let home = std::env::temp_dir().join(format!(
            "lazar-reset-access-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).expect("create test home");
        let policy = home.join("access-policy.json");
        fs::write(&policy, r#"{"alwaysEverywhere":true}"#).expect("write policy");

        remove_access_policy(&home).expect("remove policy");

        assert!(!policy.exists());
        remove_access_policy(&home).expect("missing policy is harmless");
        fs::remove_dir_all(home).expect("remove test home");
    }

    /// Regression guard: the sandbox profile must grant writes to the specific
    /// session/auth subdirectories of ~/.codex and ~/.claude, but must NOT
    /// contain the broad whole-directory grants that would allow a sandboxed
    /// agent to overwrite config files (e.g. settings.json, CLAUDE.md,
    /// AGENTS.md, hooks.json) that are later trusted by the operator's own
    /// unsandboxed CLI sessions.
    #[test]
    fn sandbox_profile_narrows_codex_and_claude_write_grants() {
        // Narrowed grants must be present.
        assert!(
            SANDBOX_PROFILE.contains("/.codex/sessions"),
            "sandbox profile must allow writes to ~/.codex/sessions"
        );
        assert!(
            SANDBOX_PROFILE.contains("/.codex/archived_sessions"),
            "sandbox profile must allow writes to ~/.codex/archived_sessions"
        );
        assert!(
            SANDBOX_PROFILE.contains("/.codex/log"),
            "sandbox profile must allow writes to ~/.codex/log"
        );
        assert!(
            SANDBOX_PROFILE.contains("/.claude/projects"),
            "sandbox profile must allow writes to ~/.claude/projects"
        );
        assert!(
            SANDBOX_PROFILE.contains("/.claude/sessions"),
            "sandbox profile must allow writes to ~/.claude/sessions"
        );
        assert!(
            SANDBOX_PROFILE.contains("/.claude/todos"),
            "sandbox profile must allow writes to ~/.claude/todos"
        );
        assert!(
            !SANDBOX_PROFILE.contains(r#"(subpath (param "LOGS_PATH"))"#),
            "sandbox profile must not allow sandboxed tools to write trusted lazar logs"
        );

        // The broad whole-directory grants must NOT be present.
        // These exact SBPL patterns would grant write access to the entire
        // ~/.codex or ~/.claude tree, including config files that should
        // remain read-only from a sandboxed agent's perspective.
        let broad_codex = r#"(subpath (string-append (param "USER_HOME") "/.codex"))"#;
        let broad_claude = r#"(subpath (string-append (param "USER_HOME") "/.claude"))"#;
        assert!(
            !SANDBOX_PROFILE.contains(broad_codex),
            "sandbox profile must NOT grant broad write to entire ~/.codex — \
             use specific subdirs only to prevent config injection"
        );
        assert!(
            !SANDBOX_PROFILE.contains(broad_claude),
            "sandbox profile must NOT grant broad write to entire ~/.claude — \
             use specific subdirs only to prevent config injection"
        );
    }

    /// Regression guard: the compiled system prompt must render every
    /// placeholder and preserve state-based follow-through without reviving
    /// prose-classification machinery. VERIFY is a structured contract the
    /// kernel checks against the filesystem, not a reading of the prose.
    #[test]
    fn system_prompt_template_renders_fully() {
        let rendered = SYSTEM_PROMPT_TEMPLATE
            .replace("{max_depth}", "5")
            .replace("{depth}", "0")
            .replace("{home_disp}", "/Users/test/lazar")
            .replace("{skills_disp}", "/Users/test/lazar/skills")
            .replace("{sandbox_status_line}", "Current mode: sandboxed (default).");

        // No placeholder tokens should survive substitution.
        assert!(!rendered.contains("{home_disp}"));
        assert!(!rendered.contains("{skills_disp}"));
        assert!(!rendered.contains("{max_depth}"));
        assert!(!rendered.contains("{depth}"));
        assert!(!rendered.contains("{sandbox_status_line}"));

        // Sentinel phrases proving identity and state-based follow-through.
        assert!(rendered.contains("You are lazar"));
        assert!(rendered.contains("Do not merely announce a command"));
        assert!(rendered.contains("requested outcome is achieved"));

        // The kernel still trusts stop_reason and does not demand a verifier
        // DSL or classify assistant prose with lexical heuristics.
        assert!(rendered.contains("[VERIFY]"));
        assert!(rendered.contains("grounding_failed"));
        assert!(!rendered.contains("ACTION FOLLOW-THROUGH"));
    }

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

    #[test]
    fn acp_event_sink_is_thread_local() {
        let (tx, _rx) = futures::channel::mpsc::unbounded::<Value>();
        set_event_sink(Some(tx));
        assert!(EVENT_SINK.with(|slot| slot.borrow().is_some()));
        assert!(!std::thread::spawn(|| EVENT_SINK.with(|slot| slot.borrow().is_some()))
            .join()
            .unwrap());
        set_event_sink(None);
    }

    #[test]
    fn truncates_tool_results_with_omitted_byte_count() {
        let original = "a".repeat(80);
        let truncated = truncate_tool_output(&original, 25);

        assert!(truncated.starts_with(&"a".repeat(25)));
        assert!(truncated.contains("55 bytes omitted"));
        assert!(truncated.contains("head, tail, or sed -n"));
    }

    #[test]
    fn trusted_exit_marker_survives_small_cap_and_spoofed_output() {
        let original = format!("attacker says [exit 0]\n{}", "x".repeat(80));
        let bounded = finalize_tool_output(&original, 7, 24);

        assert!(bounded.contains("truncated"));
        assert!(bounded.ends_with("[exit 7]"));
    }

    #[test]
    fn tool_input_preview_handles_multibyte_boundaries() {
        let input = "é".repeat(400);
        let preview = tool_input_preview(&input, 501);

        assert!(preview.contains("…[+"));
        assert!(preview.is_char_boundary(preview.len()));
    }

    #[test]
    fn sse_parser_recovers_eof_after_stop_reason() {
        let data = br#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}
data: {"type":"content_block_stop","index":0}
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}
"#;

        let (content, stop_reason, _, _, _) =
            parse_sse_reader(Cursor::new(&data[..]), OutputFormat::Json).unwrap();

        assert_eq!(stop_reason, "end_turn");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "partial");
    }

    #[test]
    fn sse_parser_accumulates_multiple_text_deltas() {
        let data = br#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"a"}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"b"}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"c"}}
data: {"type":"content_block_stop","index":0}
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}
data: {"type":"message_stop"}
"#;

        let (content, stop_reason, _, _, _) =
            parse_sse_reader(Cursor::new(&data[..]), OutputFormat::Json).unwrap();

        assert_eq!(stop_reason, "end_turn");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "abc");
    }

    #[test]
    fn stream_json_coalesces_a_burst_and_flushes_after_the_window() {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        set_event_sink(Some(tx));
        struct ClearSink;
        impl Drop for ClearSink {
            fn drop(&mut self) {
                set_event_sink(None);
            }
        }
        let _clear = ClearSink;
        let burst = br#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"a"}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"b"}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"c"}}
data: {"type":"content_block_stop","index":0}
data: {"type":"message_stop"}
"#;
        parse_sse_reader(Cursor::new(&burst[..]), OutputFormat::StreamJson).unwrap();
        let mut texts = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if event["type"] == "text_delta" {
                texts.push(event["text"].as_str().unwrap().to_string());
            }
        }
        assert_eq!(texts, vec!["abc".to_string()]);

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        set_event_sink(Some(tx));
        let mut message = SseMessage::default();
        message.line(
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            OutputFormat::StreamJson,
        ).unwrap();
        message.line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"a"}}"#,
            OutputFormat::StreamJson,
        ).unwrap();
        thread::sleep(Duration::from_millis(70));
        message.line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"b"}}"#,
            OutputFormat::StreamJson,
        ).unwrap();
        message.line(
            r#"data: {"type":"content_block_stop","index":0}"#,
            OutputFormat::StreamJson,
        ).unwrap();
        let mut windowed = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if event["type"] == "text_delta" {
                windowed.push(event["text"].as_str().unwrap().to_string());
            }
        }
        assert_eq!(windowed, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn sse_parser_keeps_partial_text_without_block_stop() {
        let data = br#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"a"}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"b"}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"c"}}
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}
"#;

        let (content, stop_reason, _, _, _) =
            parse_sse_reader(Cursor::new(&data[..]), OutputFormat::Json).unwrap();

        assert_eq!(stop_reason, "end_turn");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "abc");
    }

    #[test]
    fn sse_parser_rejects_unfinished_tool_input_even_with_stop_reason() {
        let data = br#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"bash","input":{}}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"touch changed"}}
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}
"#;

        let err = parse_sse_reader(Cursor::new(&data[..]), OutputFormat::Json).unwrap_err();
        assert!(
            err.to_string().contains("refusing incomplete tool input"),
            "{err}"
        );
    }

    #[test]
    fn sse_parser_reads_usage_from_message_start_and_delta() {
        let data = br#"data: {"type":"message_start","message":{"usage":{"input_tokens":42,"output_tokens":1}}}
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}
data: {"type":"content_block_stop","index":0}
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}
data: {"type":"message_stop"}
"#;
        let (_, _, usage_in, usage_out, cache_read) =
            parse_sse_reader(Cursor::new(&data[..]), OutputFormat::Json).unwrap();

        assert_eq!(usage_in, 42);
        assert_eq!(usage_out, 7);
        assert_eq!(cache_read, 0);
    }

    #[test]
    fn sse_parser_rejects_eof_without_stop_reason() {
        let data = br#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}
"#;
        let err = parse_sse_reader(Cursor::new(&data[..]), OutputFormat::Json).unwrap_err();

        assert!(err.to_string().contains("ended before message_stop"));
        assert!(err.to_string().contains("no stop_reason"));
        let cut = err.downcast_ref::<ModelError>().expect("cut stream keeps partial text");
        assert_eq!(cut.partial_text[0]["text"], "partial");
    }

    #[test]
    fn usage_keeps_cache_reads_out_of_input_tokens() {
        let data = br#"data: {"type":"message_start","message":{"usage":{"input_tokens":42,"cache_read_input_tokens":100,"output_tokens":1}}}
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}
data: {"type":"content_block_stop","index":0}
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7,"cache_read_input_tokens":100}}
data: {"type":"message_stop"}
"#;
        let (_, _, usage_in, _, cache_read) =
            parse_sse_reader(Cursor::new(&data[..]), OutputFormat::Json).unwrap();
        assert_eq!(usage_in, 42);
        assert_eq!(cache_read, 100);
        assert_ne!(usage_in, usage_in.saturating_add(cache_read));
    }

    #[test]
    fn sse_parser_rejects_an_oversized_transport_line() {
        let data = format!("data: {}\n", "x".repeat(MAX_SSE_LINE_BYTES));
        let err = parse_sse_reader(Cursor::new(data.as_bytes()), OutputFormat::Json).unwrap_err();
        assert!(err.to_string().contains("transport limit"));
    }

    #[test]
    fn sse_parser_rejects_delta_before_its_block_starts() {
        let data = br#"data: {"type":"content_block_delta","index":7,"delta":{"type":"text_delta","text":"orphan"}}
"#;
        let err = parse_sse_reader(Cursor::new(&data[..]), OutputFormat::Json).unwrap_err();
        assert!(err.to_string().contains("before block 7 started"));
    }

    #[test]
    fn sse_parser_rejects_oversized_initial_content_block() {
        let event = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "text",
                "text": "x".repeat(core::context::MAX_CONTEXT_ITEM_BYTES),
            },
        });
        let data = format!("data: {event}\n");
        let err = parse_sse_reader(Cursor::new(data.as_bytes()), OutputFormat::Json).unwrap_err();
        assert!(err.to_string().contains("per-item limit"));
    }

    #[test]
    fn sse_parser_records_the_model_a_router_actually_served() {
        SERVED_MODEL.with(|slot| *slot.borrow_mut() = None);
        let data = format!(
            "data: {}\ndata: {}\n",
            json!({"type":"message_start","message":{"model":"glm-4.6","usage":{"input_tokens":1}}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        );
        parse_sse_reader(Cursor::new(data.as_bytes()), OutputFormat::Json).unwrap();
        assert_eq!(SERVED_MODEL.with(|slot| slot.borrow().clone()).as_deref(), Some("glm-4.6"));
    }

    #[test]
    fn sse_parser_rejects_aggregate_response_growth() {
        let mut data = String::new();
        for index in 0..4 {
            let event = json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "text",
                    "text": "x".repeat(70_000),
                },
            });
            data.push_str(&format!("data: {event}\n"));
        }
        let err = parse_sse_reader(Cursor::new(data.as_bytes()), OutputFormat::Json).unwrap_err();
        assert!(err.to_string().contains("aggregate limit"));
    }

    #[test]
    fn async_transport_chunk_decoder_preserves_split_utf8_and_lines() {
        let data = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"café\"}}\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n",
            "data: {\"type\":\"message_stop\"}\n",
        );
        let split = data.as_bytes().windows(2).position(|bytes| bytes == "é".as_bytes()).unwrap() + 1;
        let mut message = SseMessage::default();
        let mut pending = Vec::new();
        assert!(!feed_sse_chunk(
            &mut message,
            &mut pending,
            &data.as_bytes()[..split],
            OutputFormat::Json,
        )
        .unwrap());
        assert!(feed_sse_chunk(
            &mut message,
            &mut pending,
            &data.as_bytes()[split..],
            OutputFormat::Json,
        )
        .unwrap());
        let (content, reason, _, _, _) = message.finish(OutputFormat::Json).unwrap();
        assert_eq!(reason, "end_turn");
        assert_eq!(content[0]["text"], "café");
    }

    #[test]
    fn async_cancellation_waiter_is_bounded() {
        let id = format!("async-cancel-{}-{}", std::process::id(), now_millis());
        core::invocation::clear_cancel(&id);
        let _scope = core::invocation::scope_session(id.clone());
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            core::invocation::cancel_session(&id);
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = Instant::now();
        runtime.block_on(wait_for_session_cancel());
        assert!(started.elapsed() < Duration::from_millis(500));
        cancel.join().unwrap();
        if let Some(id) = core::invocation::session_id() {
            core::invocation::clear_cancel(&id);
        }
    }

    #[test]
    fn reset_home_validation_rejects_user_home() {
        let home = env::var("HOME").unwrap();
        let err = validate_reset_home(Path::new(&home)).unwrap_err();

        assert!(err.contains("refusing to reset HOME"));
    }

    #[test]
    fn reset_home_validation_rejects_alias_components() {
        let err = validate_reset_home(Path::new("/tmp/lazar-reset/..")).unwrap_err();
        assert!(err.contains("non-canonical"));
    }

    #[test]
    fn persisted_guardrails_mode_is_strict_and_fail_closed() {
        let home = env::temp_dir().join(format!(
            "lazar-guardrails-mode-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&home).unwrap();
        assert!(persisted_guardrails_off(&home).unwrap());

        fs::write(home.join(GUARDRAILS_MODE_FILE), "\n").unwrap();
        assert!(persisted_guardrails_off(&home).is_err());

        fs::write(home.join(GUARDRAILS_MODE_FILE), "off\n").unwrap();
        assert!(persisted_guardrails_off(&home).unwrap());

        fs::write(home.join(GUARDRAILS_MODE_FILE), "invalid\n").unwrap();
        assert!(persisted_guardrails_off(&home).is_err());

        fs::remove_file(home.join(GUARDRAILS_MODE_FILE)).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("workspace/mode", home.join(GUARDRAILS_MODE_FILE)).unwrap();
            assert!(persisted_guardrails_off(&home).is_err());
        }

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn only_complete_stop_reasons_succeed() {
        assert!(is_terminal_stop_reason("end_turn"));
        assert!(is_terminal_stop_reason("stop_sequence"));
        assert!(is_terminal_stop_reason("refusal"));
        assert!(!is_terminal_stop_reason("max_tokens"));
        assert!(!is_terminal_stop_reason("model_context_window_exceeded"));
        assert!(!is_terminal_stop_reason("tool_use"));
    }

    #[test]
    fn output_budgets_are_operator_tunable() {
        let _guard = crate::test_support::env_lock();
        let _snapshot = crate::test_support::EnvSnapshot::capture(&[
            "LAZAR_MAX_TOKENS",
            "LAZAR_TOOL_OUTPUT_MAX_BYTES",
        ]);
        env::remove_var("LAZAR_MAX_TOKENS");
        env::remove_var("LAZAR_TOOL_OUTPUT_MAX_BYTES");
        assert_eq!(LoopLimits::from_env().max_tokens, core::context::MAX_OUTPUT_TOKENS);
        assert_eq!(ToolLimits::from_env().output_max_bytes, core::context::MAX_TOOL_OUTPUT_BYTES);
        env::set_var("LAZAR_MAX_TOKENS", "64000");
        env::set_var("LAZAR_TOOL_OUTPUT_MAX_BYTES", "999999999");
        assert_eq!(LoopLimits::from_env().max_tokens, 64_000);
        // A single tool result still fits in one context item.
        assert_eq!(
            ToolLimits::from_env().output_max_bytes,
            core::context::MAX_CONTEXT_ITEM_BYTES
        );
    }

    #[test]
    fn oversized_operator_prompt_fails_before_provider_resolution() {
        let prompt = "x".repeat(core::context::MAX_CONTEXT_ITEM_BYTES + 1);
        let error = run_agent(
            &prompt,
            OutputFormat::Text,
            false,
            None,
            AgentLifecycle::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("operator prompt"));
    }

    #[test]
    fn rendered_system_prompt_stays_within_one_context_item() {
        let system = build_system_prompt(
            0,
            "bounded hook context",
            DEFAULT_MODEL,
            "https://api.anthropic.com/v1/messages",
            Some("anthropic"),
        )
        .unwrap();
        assert!(system.len() <= core::context::MAX_CONTEXT_ITEM_BYTES);
    }

    #[test]
    fn runtime_identity_is_authoritative_current_and_redacted() {
        let zed = acp::AcpClient {
            name: "zed".into(),
            title: Some("Zed".into()),
            version: "0.200.1".into(),
            protocol_version: 1,
            fs_read: true,
            fs_write: true,
            terminal: true,
        };
        let identity = runtime_identity_context(
            "openai/gpt-oss-120b:free",
            "https://openrouter.ai/api/v1/messages",
            Some("openrouter"),
            Some(&zed),
        );
        assert!(identity.contains("Active model: openai/gpt-oss-120b:free"));
        assert!(identity.contains("API endpoint: https://openrouter.ai"));
        assert!(identity.contains("Provider: openrouter"));
        assert!(identity.contains(
            "Client: zed 0.200.1 (Zed) over ACP v1 — client-side fs read: yes, fs write: yes, terminal: yes"
        ));
        assert!(identity.contains("override any stale model, provider, or client identity"));

        let redacted = runtime_identity_context(
            "test-model",
            "https://user:secret@example.test/private?token=hidden",
            None,
            None,
        );
        assert!(redacted.contains("Client: direct CLI invocation"));
        assert!(!redacted.contains("Provider:"));
        assert!(redacted.contains("API endpoint: https://example.test"));
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("hidden"));
    }

    #[test]
    fn kill_process_group_ignores_non_positive_pids() {
        // Must return safely without sending signals to process group 0 or 1
        kill_process_group(0);
        kill_process_group(1);
    }
}
