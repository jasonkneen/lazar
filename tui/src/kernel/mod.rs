//! Long-lived ACP transport for the Lazar kernel.
//!
//! The worker owns `lazar acp`, writes newline-delimited JSON-RPC requests,
//! and converts both native `session/update` notifications and Lazar's
//! `_lazar/event` extension into a small UI-facing event vocabulary. The UI
//! thread never blocks on the model or child-process I/O.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct KernelConfig {
    pub binary: PathBuf,
    pub lazar_home: PathBuf,
    pub cwd: PathBuf,
    pub additional_dirs: Vec<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub no_sandbox: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum KernelEvent {
    Status(String),
    Ready {
        session_id: String,
    },
    HistoryUser(String),
    HistoryAssistant(String),
    TextDelta(String),
    ToolStart {
        id: String,
        command: String,
    },
    ToolResult {
        id: String,
        output: String,
        exit_code: Option<i32>,
    },
    Lazar(Value),
    TurnFinished {
        stop_reason: String,
    },
    Sessions(Vec<SessionRow>),
    Notice(String),
    Error(String),
    /// The worker sent a prompt that had been sitting in its queue.
    PromptAccepted(String),
}

#[derive(Debug)]
enum KernelCommand {
    Prompt(String),
    DropQueued,
    Cancel,
    Restart(KernelConfig),
    LoadSession(String),
    ListSessions,
    Shutdown,
}

pub struct KernelHandle {
    commands: Sender<KernelCommand>,
    pub events: Receiver<KernelEvent>,
    worker: Option<thread::JoinHandle<()>>,
}

impl KernelHandle {
    pub fn spawn(config: KernelConfig) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("lazartui-kernel".into())
            .spawn(move || worker(config, command_rx, event_tx))
            .expect("spawn Lazar ACP worker");
        Self {
            commands: command_tx,
            events: event_rx,
            worker: Some(worker),
        }
    }

    pub fn prompt(&self, text: String) -> Result<(), String> {
        self.commands
            .send(KernelCommand::Prompt(text))
            .map_err(|e| e.to_string())
    }

    pub fn drop_queued(&self) -> Result<(), String> {
        self.commands
            .send(KernelCommand::DropQueued)
            .map_err(|e| e.to_string())
    }

    pub fn cancel(&self) -> Result<(), String> {
        self.commands
            .send(KernelCommand::Cancel)
            .map_err(|e| e.to_string())
    }

    pub fn restart(&self, config: KernelConfig) -> Result<(), String> {
        self.commands
            .send(KernelCommand::Restart(config))
            .map_err(|e| e.to_string())
    }

    pub fn load_session(&self, id: String) -> Result<(), String> {
        self.commands
            .send(KernelCommand::LoadSession(id))
            .map_err(|e| e.to_string())
    }

    pub fn list_sessions(&self) -> Result<(), String> {
        self.commands
            .send(KernelCommand::ListSessions)
            .map_err(|e| e.to_string())
    }
}

impl Drop for KernelHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(KernelCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

enum Wire {
    Json(Value),
    Stderr(String),
    Eof,
}

struct KernelProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    wire: Receiver<Wire>,
}

impl KernelProcess {
    fn start(config: &KernelConfig) -> Result<Self, String> {
        let mut command = Command::new(&config.binary);
        command.arg("acp").arg("--cwd").arg(&config.cwd);
        if let Some(provider) = config.provider.as_deref() {
            command.arg("--provider").arg(provider);
        }
        if let Some(model) = config.model.as_deref() {
            command.arg("--model").arg(model);
        }
        if config.no_sandbox {
            command.arg("--no-sandbox");
        } else {
            command.arg("--sandbox");
        }
        for dir in &config.additional_dirs {
            command.arg("--add-dir").arg(dir);
        }
        let mut child = command
            .env("LAZAR_HOME", &config.lazar_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", config.binary.display()))?;
        let stdin = child.stdin.take().ok_or("kernel stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("kernel stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("kernel stderr unavailable")?;
        let (tx, rx) = mpsc::channel();
        let out_tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) if line.trim().is_empty() => {}
                    Ok(line) => match serde_json::from_str(&line) {
                        Ok(value) => {
                            if out_tx.send(Wire::Json(value)).is_err() {
                                return;
                            }
                        }
                        Err(err) => {
                            let _ = out_tx.send(Wire::Stderr(format!(
                                "invalid ACP JSON ({err}): {}",
                                line.chars().take(240).collect::<String>()
                            )));
                        }
                    },
                    Err(_) => break,
                }
            }
            let _ = out_tx.send(Wire::Eof);
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(Wire::Stderr(line)).is_err() {
                    return;
                }
            }
        });
        Ok(Self {
            child,
            stdin: Some(stdin),
            wire: rx,
        })
    }

    fn send(&mut self, value: &Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("kernel stdin closed")?;
        serde_json::to_writer(&mut *stdin, value).map_err(|e| e.to_string())?;
        stdin.write_all(b"\n").map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())
    }

    fn stop(&mut self, session_id: Option<&str>) {
        if let Some(id) = session_id {
            let _ = self.send(&json!({"jsonrpc":"2.0", "method":"session/cancel", "params":{"sessionId":id}}));
        }
        // EOF asks ACP to cancel and drain all workers, then fire session-end.
        // Killing ACP here would bypass cleanup of separately grouped tools.
        self.stdin.take();
        let _ = self.child.wait();
    }

}

#[derive(Debug, Clone, Copy)]
enum Pending {
    Initialize,
    NewSession,
    LoadSession { fallback_new: bool },
    Prompt,
    ListSessions,
}

struct WorkerState {
    config: KernelConfig,
    process: KernelProcess,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    session_id: Option<String>,
    ready: bool,
    turn_running: bool,
    queued_prompt: Option<String>,
}

/// The `initialize` handshake. lazartui identifies itself and declares,
/// truthfully, that it offers the agent no client-side file or terminal
/// access: every tool call runs inside the kernel's own sandbox. The kernel
/// records this and tells the model who is driving it.
fn initialize_params() -> Value {
    json!({
        "protocolVersion": 1,
        "clientInfo": {
            "name": "lazartui",
            "title": "Lazar TUI",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
        },
    })
}

impl WorkerState {
    fn start(config: KernelConfig) -> Result<Self, String> {
        let process = KernelProcess::start(&config)?;
        let session_id = config.session_id.clone();
        let mut state = Self {
            config,
            process,
            next_id: 1,
            pending: HashMap::new(),
            session_id,
            ready: false,
            turn_running: false,
            queued_prompt: None,
        };
        state.request("initialize", initialize_params(), Pending::Initialize)?;
        Ok(state)
    }

    fn request(&mut self, method: &str, params: Value, pending: Pending) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.process.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }))?;
        self.pending.insert(id, pending);
        Ok(id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.process
            .send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn session_params(&self, id: Option<&str>) -> Value {
        let mut params = json!({
            "cwd": self.config.cwd,
            "mcpServers": [],
            "additionalDirectories": self.config.additional_dirs,
        });
        if let Some(id) = id {
            params["sessionId"] = json!(id);
        }
        params
    }

    fn open_after_initialize(&mut self) -> Result<(), String> {
        if let Some(id) = self.session_id.clone() {
            let params = self.session_params(Some(&id));
            self.request(
                "session/load",
                params,
                Pending::LoadSession { fallback_new: true },
            )?;
        } else {
            let params = self.session_params(None);
            self.request("session/new", params, Pending::NewSession)?;
        }
        Ok(())
    }

    fn prompt_now(&mut self, text: String) -> Result<(), String> {
        let id = self
            .session_id
            .clone()
            .ok_or("kernel session is not ready")?;
        self.turn_running = true;
        self.request(
            "session/prompt",
            json!({"sessionId": id, "prompt": [{"type": "text", "text": text}]}),
            Pending::Prompt,
        )?;
        Ok(())
    }
}

fn worker(
    mut config: KernelConfig,
    commands: Receiver<KernelCommand>,
    events: Sender<KernelEvent>,
) {
    let mut state = loop {
        match WorkerState::start(config.clone()) {
            Ok(state) => break state,
            Err(err) => {
                let _ = events.send(KernelEvent::Error(err));
                loop {
                    match commands.recv() {
                        Ok(KernelCommand::Restart(next)) => { config = next; break; }
                        Ok(KernelCommand::Shutdown) | Err(_) => return,
                        _ => { let _ = events.send(KernelEvent::Notice("kernel disconnected; restart to continue".into())); }
                    }
                }
            }
        }
    };
    let _ = events.send(KernelEvent::Status("connecting to kernel".into()));
    let mut running = true;
    let mut connected = true;
    while running {
        while let Ok(command) = commands.try_recv() {
            match command {
                KernelCommand::Prompt(text) => {
                    if state.ready && !state.turn_running {
                        if let Err(err) = state.prompt_now(text) {
                            let _ = events.send(KernelEvent::Error(err));
                        }
                    } else {
                        state.queued_prompt = Some(text);
                    }
                }
                KernelCommand::DropQueued => {
                    state.queued_prompt = None;
                }
                KernelCommand::Cancel => {
                    if let Some(id) = state.session_id.clone() {
                        let _ = state.notify("session/cancel", json!({"sessionId": id}));
                        let _ = events.send(KernelEvent::Status("cancelling turn".into()));
                    }
                }
                KernelCommand::Restart(next) => {
                    state.process.stop(state.session_id.as_deref());
                    config = next;
                    match WorkerState::start(config.clone()) {
                        Ok(next_state) => {
                            state = next_state;
                            connected = true;
                            let _ = events.send(KernelEvent::Status("restarting kernel".into()));
                        }
                        Err(err) => {
                            connected = false;
                            state.ready = false;
                            state.turn_running = false;
                            let _ = events.send(KernelEvent::Error(err));
                        }
                    }
                }
                KernelCommand::LoadSession(id) => {
                    state.ready = false;
                    state.session_id = Some(id.clone());
                    let params = state.session_params(Some(&id));
                    if let Err(err) = state.request(
                        "session/load",
                        params,
                        Pending::LoadSession {
                            fallback_new: false,
                        },
                    ) {
                        let _ = events.send(KernelEvent::Error(err));
                    }
                }
                KernelCommand::ListSessions => {
                    if let Err(err) =
                        state.request("session/list", json!({}), Pending::ListSessions)
                    {
                        let _ = events.send(KernelEvent::Error(err));
                    }
                }
                KernelCommand::Shutdown => {
                    running = false;
                    break;
                }
            }
        }
        if !running {
            break;
        }
        if !connected {
            thread::sleep(Duration::from_millis(25));
            continue;
        }
        match state.process.wire.recv_timeout(Duration::from_millis(25)) {
            Ok(Wire::Json(message)) => handle_wire(&mut state, message, &events),
            Ok(Wire::Stderr(line)) => {
                if line.starts_with("[lazar] seeded ") {
                    let _ = events.send(KernelEvent::Notice(line));
                } else if line.contains("SANDBOX DISABLED") || line.contains("[lazar acp]") {
                    let _ = events.send(KernelEvent::Status(line));
                } else if !line.trim().is_empty() {
                    let _ = events.send(KernelEvent::Notice(line));
                }
            }
            Ok(Wire::Eof) => {
                let _ = events.send(KernelEvent::Error("kernel connection closed".into()));
                state.ready = false;
                state.turn_running = false;
                connected = false;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                state.ready = false;
                state.turn_running = false;
                connected = false;
            },
        }
    }
    state.process.stop(state.session_id.as_deref());
}

fn handle_wire(state: &mut WorkerState, message: Value, events: &Sender<KernelEvent>) {
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        match method {
            "session/update" => {
                if let Some(event) =
                    decode_session_update(&message["params"]["update"], state.turn_running)
                {
                    let _ = events.send(event);
                }
            }
            "_lazar/event" => {
                if let Some(event) = message["params"].get("event") {
                    let _ = events.send(KernelEvent::Lazar(event.clone()));
                }
            }
            _ => {}
        }
        return;
    }

    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        return;
    };
    let Some(pending) = state.pending.remove(&id) else {
        return;
    };
    if let Some(error) = message.get("error") {
        let detail = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("ACP request failed");
        if matches!(pending, Pending::ListSessions) {
            let _ = events.send(KernelEvent::Notice(detail.to_string()));
        } else if matches!(pending, Pending::LoadSession { fallback_new: true }) {
            state.session_id = None;
            let params = state.session_params(None);
            if let Err(err) = state.request("session/new", params, Pending::NewSession) {
                let _ = events.send(KernelEvent::Error(err));
            }
        } else {
            state.ready = !matches!(pending, Pending::LoadSession { .. });
            state.turn_running = false;
            let _ = events.send(KernelEvent::Error(detail.to_string()));
        }
        return;
    }
    let result = &message["result"];
    match pending {
        Pending::Initialize => {
            if let Err(err) = state.open_after_initialize() {
                let _ = events.send(KernelEvent::Error(err));
            }
        }
        Pending::NewSession => {
            if let Some(id) = result.get("sessionId").and_then(Value::as_str) {
                state.session_id = Some(id.to_string());
                state.ready = true;
                let _ = events.send(KernelEvent::Ready {
                    session_id: id.to_string(),
                });
                send_queued(state, events);
            }
        }
        Pending::LoadSession { .. } => {
            if let Some(id) = state.session_id.clone() {
                state.ready = true;
                let _ = events.send(KernelEvent::Ready { session_id: id });
                send_queued(state, events);
            }
        }
        Pending::Prompt => {
            state.turn_running = false;
            let reason = result
                .get("stopReason")
                .and_then(Value::as_str)
                .unwrap_or("end_turn");
            let _ = events.send(KernelEvent::TurnFinished {
                stop_reason: reason.to_string(),
            });
            send_queued(state, events);
        }
        Pending::ListSessions => {
            let rows = result
                .get("sessions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|row| {
                    Some(SessionRow {
                        id: row.get("sessionId")?.as_str()?.to_string(),
                        cwd: row
                            .get("cwd")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        title: row.get("title").and_then(Value::as_str).map(str::to_string),
                        updated_at: row
                            .get("updatedAt")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect();
            let _ = events.send(KernelEvent::Sessions(rows));
        }
    }
}

fn send_queued(state: &mut WorkerState, events: &Sender<KernelEvent>) {
    if state.ready && !state.turn_running {
        if let Some(prompt) = state.queued_prompt.take() {
            match state.prompt_now(prompt.clone()) {
                Ok(()) => {
                    let _ = events.send(KernelEvent::PromptAccepted(prompt));
                }
                Err(err) => {
                    let _ = events.send(KernelEvent::Error(err));
                }
            }
        }
    }
}

fn decode_session_update(update: &Value, turn_running: bool) -> Option<KernelEvent> {
    match update.get("sessionUpdate")?.as_str()? {
        "user_message_chunk" => Some(KernelEvent::HistoryUser(content_text(&update["content"]))),
        "agent_message_chunk" => {
            let text = content_text(&update["content"]);
            Some(if turn_running {
                KernelEvent::TextDelta(text)
            } else {
                KernelEvent::HistoryAssistant(text)
            })
        }
        "tool_call" => {
            let id = update.get("toolCallId")?.as_str()?.to_string();
            let command = update
                .get("rawInput")
                .and_then(|v| v.get("command"))
                .and_then(Value::as_str)
                .or_else(|| update.get("title").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            Some(KernelEvent::ToolStart { id, command })
        }
        "tool_call_update" => {
            let id = update.get("toolCallId")?.as_str()?.to_string();
            let raw = update.get("rawOutput").unwrap_or(&Value::Null);
            let output = raw
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| tool_content_text(update.get("content")));
            let exit_code = raw
                .get("exit_code")
                .and_then(Value::as_i64)
                .map(|n| n as i32);
            Some(KernelEvent::ToolResult {
                id,
                output,
                exit_code,
            })
        }
        _ => None,
    }
}

fn content_text(content: &Value) -> String {
    content
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn tool_content_text(content: Option<&Value>) -> String {
    let Some(items) = content.and_then(Value::as_array) else {
        return String::new();
    };
    items
        .iter()
        .filter_map(|item| {
            item.get("content")
                .and_then(|v| v.get("text"))
                .or_else(|| item.get("text"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    fn fake_kernel_root() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lazartui-acp-process-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create test root");
        root
    }

    fn write_fake_acp_kernel(root: &std::path::Path) -> PathBuf {
        let script = root.join("fake-lazar");
        fs::write(
            &script,
            r####"#!/usr/bin/env bash
set -u
LOG="$LAZAR_HOME/requests.jsonl"
while IFS= read -r line; do
    printf '%s\n' "$line" >>"$LOG"
    method=$(printf '%s\n' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
    id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$method" in
        initialize)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"fake-lazar","version":"1"}}}\n' "$id"
            ;;
        session/new|session/load)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"session-test"}}\n' "$id"
            ;;
        session/prompt)
            echo "ordinary diagnostic while prompt is running" >&2
            printf '%s\n' \
                '{"jsonrpc":"2.0","method":"_lazar/event","params":{"sessionId":"session-test","event":{"type":"invoke_start"}}}' \
                '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-test","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}' \
                '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-test","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"bash","rawInput":{"command":"printf test"}}}}' \
                '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-test","update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","rawOutput":{"content":"test\n","exit_code":0}}}}'
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
        session/list)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessions":[{"sessionId":"session-test","cwd":"/tmp","title":"Test","updatedAt":"now"}]}}\n' "$id"
            ;;
    esac
done
"####,
        )
        .expect("write fake ACP kernel");
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        script
    }

    #[test]
    #[ignore = "requires LAZAR_TEST_KERNEL and permission to bind loopback"]
    fn real_tui_acp_active_tool_cancel_shutdown_and_restart() {
        use std::io::Read;
        use std::net::TcpListener;
        let kernel = std::env::var("LAZAR_TEST_KERNEL").expect("build src/ and set LAZAR_TEST_KERNEL");
        for action in ["cancel", "shutdown", "restart"] {
            let root = fake_kernel_root();
            fs::create_dir_all(root.join("hooks/session-end.d")).unwrap();
            let hook = root.join("hooks/session-end.d/end.sh");
            fs::write(&hook, "#!/bin/bash\nprintf done > \"$LAZAR_HOME/closed\"\n").unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").expect("loopback required for real provider transport test");
            let address = listener.local_addr().unwrap();
            let wrapper = root.join("real-kernel");
            fs::write(&wrapper, format!("#!/bin/bash\nunset LAZAR_PROVIDER LAZAR_MODEL LAZAR_SESSION_ID\nexport ANTHROPIC_API_KEY=test-key ANTHROPIC_BASE_URL=http://{address} GH_TOKEN=test-credential\nexec \"{kernel}\" \"$@\"\n")).unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
            let command = "test \"$GH_TOKEN\" = test-credential || exit 23; sleep 60 & child=$!; printf '%s %s\\n' \"$$\" \"$child\" > \"$LAZAR_HOME/active-pid.tmp\"; mv \"$LAZAR_HOME/active-pid.tmp\" \"$LAZAR_HOME/active-pid\"; wait";
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
                let headers = String::from_utf8_lossy(&request);
                let length = headers.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|value| value.trim().parse::<usize>().unwrap())).unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let events = [
                    json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"active","name":"bash","input":{}}}),
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":json!({"command":command}).to_string()}}),
                    json!({"type":"content_block_stop","index":0}),
                    json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"later","name":"bash","input":{}}}),
                    json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":json!({"command":"touch \"$LAZAR_HOME/should-not-run\""}).to_string()}}),
                    json!({"type":"content_block_stop","index":1}),
                    json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
                    json!({"type":"message_stop"}),
                ];
                let body = events.iter().map(|event| format!("event: {}\ndata: {}\n\n", event["type"].as_str().unwrap(), event)).collect::<String>();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            });
            let config = KernelConfig { binary: wrapper, lazar_home: root.clone(), cwd: root.clone(), additional_dirs: vec![], provider: None, model: None, session_id: None, no_sandbox: true };
            let handle = KernelHandle::spawn(config.clone());
            handle.prompt("run the test commands".into()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(8);
            while !root.join("active-pid").exists() {
                assert!(Instant::now() < deadline, "active command did not start");
                thread::sleep(Duration::from_millis(10));
            }
            let active_pids: Vec<u32> = fs::read_to_string(root.join("active-pid"))
                .unwrap().split_whitespace().map(|pid| pid.parse().unwrap()).collect();
            assert_eq!(active_pids.len(), 2, "record both shell and child");
            if action == "cancel" {
                handle.cancel().unwrap();
                loop {
                    assert!(Instant::now() < deadline);
                    if let Ok(KernelEvent::TurnFinished { stop_reason }) = handle.events.recv_timeout(Duration::from_millis(50)) {
                        assert_eq!(stop_reason, "cancelled"); break;
                    }
                }
            } else if action == "restart" {
                handle.restart(config).unwrap();
                while !root.join("closed").exists() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(10));
                }
            }
            drop(handle);
            server.join().unwrap();
            assert!(root.join("closed").exists(), "session-end did not fire for {action}");
            assert!(!root.join("should-not-run").exists(), "subsequent tool ran after {action}");
            let cleanup_deadline = Instant::now() + Duration::from_secs(3);
            for pid in active_pids {
                while Command::new("/bin/kill").args(["-0", &pid.to_string()])
                    .stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success() {
                    assert!(Instant::now() < cleanup_deadline, "process {pid} survived {action}");
                    thread::sleep(Duration::from_millis(10));
                }
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn eof_keeps_restart_command_receiver_alive() {
        let root = fake_kernel_root();
        let mut config = KernelConfig {
            binary: PathBuf::from("/usr/bin/false"), lazar_home: root.clone(), cwd: root.clone(),
            additional_dirs: vec![], provider: None, model: None, session_id: None, no_sandbox: false,
        };
        let handle = KernelHandle::spawn(config.clone());
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline);
            if matches!(handle.events.recv_timeout(Duration::from_secs(1)).unwrap(), KernelEvent::Error(_)) { break; }
        }
        config.binary = write_fake_acp_kernel(&root);
        handle.restart(config).unwrap();
        loop {
            assert!(Instant::now() < deadline);
            if matches!(handle.events.recv_timeout(Duration::from_secs(1)).unwrap(), KernelEvent::Ready { .. }) { break; }
        }
        drop(handle);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initialize_identifies_this_client_honestly() {
        let params = initialize_params();
        assert_eq!(params["protocolVersion"], 1);
        assert_eq!(params["clientInfo"]["name"], "lazartui");
        assert_eq!(params["clientInfo"]["version"], env!("CARGO_PKG_VERSION"));
        // The TUI has no fs/terminal surface of its own to offer the agent.
        assert_eq!(params["clientCapabilities"]["fs"]["readTextFile"], false);
        assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], false);
        assert_eq!(params["clientCapabilities"]["terminal"], false);
    }

    #[test]
    fn decodes_native_text_and_tools() {
        assert_eq!(
            decode_session_update(
                &json!({
                    "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}
                }),
                true
            ),
            Some(KernelEvent::TextDelta("hi".into()))
        );
        assert_eq!(
            decode_session_update(
                &json!({
                    "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "bash", "rawInput": {"command": "pwd"}
                }),
                true
            ),
            Some(KernelEvent::ToolStart {
                id: "t1".into(),
                command: "pwd".into()
            })
        );
        assert_eq!(
            decode_session_update(
                &json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": "t1",
                    "rawOutput": {"content": "/tmp\n", "exit_code": 0}
                }),
                true
            ),
            Some(KernelEvent::ToolResult {
                id: "t1".into(),
                output: "/tmp\n".into(),
                exit_code: Some(0)
            })
        );
    }

    #[test]
    fn history_is_distinct_from_live_text() {
        let update = json!({"sessionUpdate": "agent_message_chunk", "content": {"text": "old"}});
        assert_eq!(
            decode_session_update(&update, false),
            Some(KernelEvent::HistoryAssistant("old".into()))
        );
    }

    #[test]
    fn worker_drives_a_real_acp_subprocess_and_decodes_frames() {
        let root = fake_kernel_root();
        let binary = write_fake_acp_kernel(&root);
        let extra = root.join("extra");
        fs::create_dir_all(&extra).unwrap();
        let handle = KernelHandle::spawn(KernelConfig {
            binary,
            lazar_home: root.clone(),
            cwd: root.clone(),
            additional_dirs: vec![extra.clone()],
            provider: Some("test-provider".into()),
            model: Some("test-model".into()),
            session_id: None,
            no_sandbox: false,
        });

        // Exercise the queue before initialize/session-new completes.
        handle.prompt("queued prompt".into()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        let mut text = false;
        let mut tool_start = false;
        let mut tool_result = false;
        let mut raw_event = false;
        let mut finished = false;
        while Instant::now() < deadline && !finished {
            let event = match handle.events.recv_timeout(Duration::from_millis(250)) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => panic!("worker event channel closed"),
            };
            match event {
                KernelEvent::Ready { session_id } => ready = session_id == "session-test",
                KernelEvent::TextDelta(value) => text = value == "hello",
                KernelEvent::ToolStart { id, command } => {
                    tool_start = id == "t1" && command == "printf test";
                }
                KernelEvent::ToolResult {
                    id,
                    output,
                    exit_code,
                } => {
                    tool_result = id == "t1" && output == "test\n" && exit_code == Some(0);
                }
                KernelEvent::Lazar(value) => raw_event = value["type"] == "invoke_start",
                KernelEvent::TurnFinished { stop_reason } => {
                    finished = stop_reason == "end_turn";
                }
                KernelEvent::Error(error) => panic!("worker error: {error}"),
                _ => {}
            }
        }
        assert!(
            ready && text && tool_start && tool_result && raw_event && finished,
            "ready={ready} text={text} tool_start={tool_start} tool_result={tool_result} raw_event={raw_event} finished={finished}"
        );

        handle.list_sessions().unwrap();
        let sessions = loop {
            match handle.events.recv_timeout(Duration::from_secs(2)).unwrap() {
                KernelEvent::Sessions(rows) => break rows,
                KernelEvent::Error(error) => panic!("worker error: {error}"),
                _ => {}
            }
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "session-test");

        handle.cancel().unwrap();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(2)).unwrap() {
                KernelEvent::Status(status) if status == "cancelling turn" => break,
                KernelEvent::Error(error) => panic!("worker error: {error}"),
                _ => {}
            }
        }
        thread::sleep(Duration::from_millis(50));
        drop(handle);

        let requests: Vec<Value> = fs::read_to_string(root.join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(requests[0]["method"], "initialize");
        assert_eq!(requests[0]["params"]["clientInfo"]["name"], "lazartui");
        assert_eq!(requests[1]["method"], "session/new");
        assert_eq!(
            requests[1]["params"]["cwd"],
            root.to_string_lossy().as_ref()
        );
        assert_eq!(
            requests[1]["params"]["additionalDirectories"][0],
            extra.to_string_lossy().as_ref()
        );
        let prompt = requests
            .iter()
            .find(|request| request["method"] == "session/prompt")
            .expect("prompt request");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "queued prompt");
        assert!(requests
            .iter()
            .any(|request| request["method"] == "session/list"));
        assert!(requests
            .iter()
            .any(|request| request["method"] == "session/cancel"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dropping_a_prompt_queued_before_ready_sends_nothing() {
        let root = fake_kernel_root();
        let script = root.join("fake-lazar");
        fs::write(
            &script,
            r#"#!/usr/bin/env bash
set -u
LOG="$LAZAR_HOME/requests.jsonl"
while IFS= read -r line; do
    printf '%s\n' "$line" >>"$LOG"
    method=$(printf '%s\n' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
    id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$method" in
        initialize)
            while [ ! -f "$LAZAR_HOME/go" ]; do sleep 0.02; done
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"fake-lazar","version":"1"}}}\n' "$id"
            ;;
        session/new|session/load)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"session-test"}}\n' "$id"
            ;;
        session/prompt)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
            ;;
    esac
done
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        let handle = KernelHandle::spawn(KernelConfig {
            binary: script,
            lazar_home: root.clone(),
            cwd: root.clone(),
            additional_dirs: vec![],
            provider: None,
            model: None,
            session_id: None,
            no_sandbox: false,
        });
        handle.prompt("do not send".into()).unwrap();
        handle.drop_queued().unwrap();
        thread::sleep(Duration::from_millis(150));
        fs::write(root.join("go"), "1").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut ready = false;
        while Instant::now() < deadline && !ready {
            match handle.events.recv_timeout(Duration::from_millis(200)) {
                Ok(KernelEvent::Ready { .. }) => ready = true,
                Ok(KernelEvent::Error(error)) => panic!("worker error: {error}"),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => panic!("worker event channel closed"),
            }
        }
        assert!(ready, "session never became ready");
        thread::sleep(Duration::from_millis(150));
        let requests = fs::read_to_string(root.join("requests.jsonl")).unwrap_or_default();
        assert!(
            !requests.contains("session/prompt"),
            "dropped queue must not become a prompt: {requests}"
        );
        drop(handle);
        fs::remove_dir_all(root).unwrap();
    }
}
