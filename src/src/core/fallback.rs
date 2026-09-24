//! Fallback to another coding agent over the Agent Client Protocol when the
//! model provider is down: Claude Code (claude-code-acp), Codex (codex-acp),
//! Gemini (`gemini --acp`), OpenCode (`opencode acp`). The kernel acts as the
//! ACP *client*: it hands the agent a transcript of the conversation, streams
//! the agent's text and tool calls as ordinary lazar events, and approves
//! every permission request (operator's choice: always allow).

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

/// Agents tried in order when `providers.toml` has no `[fallback]` table.
pub(crate) fn default_agents() -> Vec<Vec<String>> {
    [
        vec!["claude-code-acp"],
        vec!["codex-acp"],
        vec!["opencode", "acp"],
        vec!["gemini", "--acp"],
    ]
    .into_iter()
    .map(|argv| argv.into_iter().map(str::to_string).collect())
    .collect()
}

/// Agents whose program resolves on PATH (or is a path that exists), in order.
pub(crate) fn available(agents: &[Vec<String>]) -> Vec<Vec<String>> {
    agents.iter().filter(|argv| argv.first().is_some_and(|p| on_path(p))).cloned().collect()
}

/// `initialize` + `session/new` must answer within this; a hung agent is
/// skipped for the next one. The prompt itself runs until done or cancelled.
const HANDSHAKE: Duration = Duration::from_secs(30);

fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

pub(crate) const HANDOFF: &str = "You are taking over a task from another coding agent (lazar) whose model provider is temporarily unavailable. The conversation so far is below. Continue from where it left off: do what the operator most recently asked, then report plainly what you did and found.";

pub(crate) struct Outcome {
    pub agent: String,
    pub text: String,
    pub tool_calls: u32,
}

struct Rpc {
    stdin: ChildStdin,
    lines: Receiver<Value>,
    next_id: u64,
}

impl Rpc {
    fn send(&mut self, value: Value) -> Result<(), String> {
        writeln!(self.stdin, "{value}").and_then(|_| self.stdin.flush()).map_err(|e| format!("agent stdin: {e}"))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<u64, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        Ok(id)
    }
}

/// Answer a request the agent makes of us. Permission: the most permissive
/// allow option. Anything else (fs/terminal) we did not advertise.
fn answer_agent_request(rpc: &mut Rpc, message: &Value) -> Result<(), String> {
    let id = message["id"].clone();
    if message["method"] == "session/request_permission" {
        let options = message["params"]["options"].as_array().cloned().unwrap_or_default();
        let pick = ["allow_always", "allow_once"]
            .iter()
            .find_map(|kind| options.iter().find(|o| o["kind"] == *kind))
            .or_else(|| options.first())
            .and_then(|o| o["optionId"].as_str())
            .unwrap_or("allow")
            .to_string();
        return rpc.send(json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": {"outcome": "selected", "optionId": pick}}}));
    }
    rpc.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "not supported by lazar"}}))
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(content_text).collect::<Vec<_>>().join(""),
        Value::Object(_) => {
            if let Some(text) = content["text"].as_str() {
                text.to_string()
            } else {
                content_text(&content["content"])
            }
        }
        _ => String::new(),
    }
}

/// Wait for the response to `id`, handling notifications and agent requests
/// meanwhile. `cancelled` is polled so the operator can stop the turn.
fn await_response(
    rpc: &mut Rpc,
    id: u64,
    emit: &mut dyn FnMut(Value),
    text: &mut String,
    tool_calls: &mut u32,
    cancelled: &dyn Fn() -> bool,
    session: Option<&str>,
    deadline: Option<std::time::Instant>,
) -> Result<Value, String> {
    loop {
        if deadline.is_some_and(|d| std::time::Instant::now() > d) {
            return Err("fallback agent did not answer the ACP handshake".into());
        }
        if cancelled() {
            if let Some(session) = session {
                let _ = rpc.send(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}));
            }
            return Err("cancelled".into());
        }
        let message = match rpc.lines.recv_timeout(Duration::from_millis(200)) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Err("fallback agent exited".into()),
        };
        if message.get("method").is_some() && message.get("id").is_some() {
            answer_agent_request(rpc, &message)?;
            continue;
        }
        if message["method"] == "session/update" {
            let update = &message["params"]["update"];
            match update["sessionUpdate"].as_str().unwrap_or("") {
                "agent_message_chunk" => {
                    let chunk = content_text(&update["content"]);
                    if !chunk.is_empty() {
                        text.push_str(&chunk);
                        emit(json!({"type": "text_delta", "text": chunk}));
                    }
                }
                "tool_call" => {
                    *tool_calls += 1;
                    let command = update["rawInput"]["command"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| update["rawInput"]["command"].as_array().map(|a| {
                            a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")
                        }))
                        .unwrap_or_else(|| update["title"].as_str().unwrap_or("tool").to_string());
                    emit(json!({"type": "tool_use", "id": update["toolCallId"], "name": "bash", "input": {"command": command}}));
                }
                "tool_call_update" => {
                    let status = update["status"].as_str().unwrap_or("");
                    if matches!(status, "completed" | "failed") {
                        emit(json!({
                            "type": "tool_result",
                            "tool_use_id": update["toolCallId"],
                            "content": content_text(&update["content"]),
                            "exit_code": if status == "failed" { 1 } else { 0 },
                        }));
                    }
                }
                _ => {}
            }
            continue;
        }
        if message["id"] == json!(id) {
            if let Some(error) = message.get("error") {
                return Err(format!("fallback agent error: {}", error["message"].as_str().unwrap_or(&error.to_string())));
            }
            return Ok(message["result"].clone());
        }
    }
}

fn spawn(argv: &[String], cwd: &Path) -> Result<(Child, Rpc), String> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        // Claude Code refuses to start when it thinks it is nested.
        .env_remove("CLAUDECODE")
        // The provider these point at is the one that just failed; the
        // fallback agent must use its own login and endpoint.
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", argv[0]))?;
    let stdin = child.stdin.take().ok_or("no agent stdin")?;
    let stdout = child.stdout.take().ok_or("no agent stdout")?;
    let (tx, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if tx.send(value).is_err() {
                    break;
                }
            }
        }
    });
    Ok((child, Rpc { stdin, lines, next_id: 0 }))
}

/// Run one prompt through `argv`'s ACP agent in `cwd`.
pub(crate) fn run(
    argv: &[String],
    cwd: &Path,
    prompt: &str,
    emit: &mut dyn FnMut(Value),
    cancelled: &dyn Fn() -> bool,
) -> Result<Outcome, String> {
    let (mut child, mut rpc) = spawn(argv, cwd)?;
    let mut text = String::new();
    let mut tool_calls = 0;
    let result = (|| {
        let id = rpc.request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}}),
        )?;
        let deadline = Some(std::time::Instant::now() + HANDSHAKE);
        await_response(&mut rpc, id, emit, &mut text, &mut tool_calls, cancelled, None, deadline)?;
        let id = rpc.request("session/new", json!({"cwd": cwd, "mcpServers": []}))?;
        let session = await_response(&mut rpc, id, emit, &mut text, &mut tool_calls, cancelled, None, deadline)?;
        let session_id = session["sessionId"].as_str().ok_or("agent returned no sessionId")?.to_string();
        let id = rpc.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": prompt}]}),
        )?;
        await_response(&mut rpc, id, emit, &mut text, &mut tool_calls, cancelled, Some(&session_id), None)
    })();
    let _ = child.kill();
    let _ = child.wait();
    result.map(|_| Outcome { agent: argv.join(" "), text, tool_calls })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal ACP agent in Python: asks permission, runs one "tool",
    /// streams two text chunks, ends the turn.
    const FAKE_AGENT: &str = r#"
import json, sys
def send(m): print(json.dumps(m), flush=True)
for line in sys.stdin:
    m = json.loads(line)
    meth, i = m.get("method"), m.get("id")
    if meth == "initialize": send({"jsonrpc":"2.0","id":i,"result":{"protocolVersion":1}})
    elif meth == "session/new": send({"jsonrpc":"2.0","id":i,"result":{"sessionId":"s1"}})
    elif meth == "session/prompt":
        send({"jsonrpc":"2.0","id":900,"method":"session/request_permission","params":{"options":[{"optionId":"no","kind":"reject_once"},{"optionId":"yes","kind":"allow_always"}]}})
        reply = json.loads(sys.stdin.readline())
        assert reply["result"]["outcome"]["optionId"] == "yes", reply
        up = lambda u: send({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":u}})
        up({"sessionUpdate":"tool_call","toolCallId":"t1","title":"ls","rawInput":{"command":"ls -la"}})
        up({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"a.txt"}}]})
        prompt = m["params"]["prompt"][0]["text"]
        up({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"took over: "}})
        up({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"yes" if "fix the footer" in prompt else "no"}})
        send({"jsonrpc":"2.0","id":i,"result":{"stopReason":"end_turn"}})
"#;

    #[test]
    fn drives_an_acp_agent_and_maps_its_updates() {
        let dir = std::env::temp_dir().join(format!("lazar-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("agent.py");
        std::fs::write(&script, FAKE_AGENT).unwrap();
        let argv = vec!["python3".to_string(), script.to_string_lossy().to_string()];
        let mut events = Vec::new();
        let outcome = run(&argv, &dir, "operator: fix the footer", &mut |e| events.push(e), &|| false).unwrap();
        assert_eq!(outcome.text, "took over: yes");
        assert_eq!(outcome.tool_calls, 1);
        let kinds: Vec<_> = events.iter().map(|e| e["type"].as_str().unwrap().to_string()).collect();
        assert_eq!(kinds, vec!["tool_use", "tool_result", "text_delta", "text_delta"]);
        assert_eq!(events[0]["input"]["command"], "ls -la");
        assert_eq!(events[1]["content"], "a.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_only_agents_on_path() {
        let agents = vec![vec!["definitely-not-installed-xyz".to_string()], vec!["sh".to_string()]];
        assert_eq!(available(&agents), vec![vec!["sh".to_string()]]);
    }
}
