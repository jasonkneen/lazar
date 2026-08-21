use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Harness {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Result<Value, String>>,
    root: PathBuf,
    pending: std::collections::HashMap<u64, Value>,
    notifications: Vec<Value>,
}

impl Harness {
    fn start() -> Self { Self::start_with_hook(false) }

    fn start_with_hook(active_hook: bool) -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lazar-acp-stdio-test-{}-{unique}-{sequence}",
            std::process::id()
        ));
        Self::start_at(root, active_hook)
    }

    fn start_at(root: PathBuf, active_hook: bool) -> Self {
        fs::create_dir_all(&root).expect("create isolated LAZAR_HOME");

        if active_hook {
            use std::os::unix::fs::PermissionsExt;
            fs::create_dir_all(root.join("hooks/user-prompt.d")).unwrap();
            fs::create_dir_all(root.join("hooks/session-end.d")).unwrap();
            let hook = root.join("hooks/user-prompt.d/active.sh");
            fs::write(&hook, "#!/bin/bash\necho $$ > \"$LAZAR_HOME/active-pid\"\nsleep 60 &\nwait\n").unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let end = root.join("hooks/session-end.d/end.sh");
            fs::write(&end, "#!/bin/bash\nprintf closed > \"$LAZAR_HOME/closed\"\n").unwrap();
            fs::set_permissions(&end, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_lazar"))
            .arg("acp")
            .arg("--cwd")
            .arg(&root)
            .env("LAZAR_HOME", &root)
            .env_remove("LAZAR_SESSION_ID")
            .env_remove("LAZAR_PROVIDER")
            .env("LAZAR_NO_SANDBOX", if active_hook { "1" } else { "0" })
            .env("ANTHROPIC_API_KEY", "offline-test-key")
            .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn real lazar acp process");
        let stdin = child.stdin.take().expect("ACP stdin");
        let stdout = child.stdout.take().expect("ACP stdout");
        let stderr = child.stderr.take().expect("ACP stderr");
        let (tx, replies) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        let parsed = serde_json::from_str(&line)
                            .map_err(|error| format!("invalid ACP output {line:?}: {error}"));
                        if tx.send(parsed).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(format!("ACP stdout read failed: {error}")));
                        return;
                    }
                }
            }
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("[acp child] {line}");
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            replies,
            root,
            pending: std::collections::HashMap::new(),
            notifications: Vec::new(),
        }
    }

    fn send(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        serde_json::to_writer(&mut *stdin, &value).expect("serialize request");
        stdin.write_all(b"\n").expect("write frame delimiter");
        stdin.flush().expect("flush request");
    }

    fn response(&mut self, id: u64) -> Value {
        if let Some(message) = self.pending.remove(&id) {
            assert!(message.get("error").is_none(), "ACP error response: {message}");
            return message;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let message = self
                .replies
                .recv_timeout(remaining)
                .unwrap_or_else(|error| panic!("timed out waiting for response {id}: {error}"))
                .unwrap_or_else(|error| panic!("{error}"));
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                assert_eq!(message["jsonrpc"], "2.0");
                assert!(
                    message.get("error").is_none(),
                    "ACP error response: {message}"
                );
                return message;
            }
            if let Some(other_id) = message.get("id").and_then(Value::as_u64) {
                self.pending.insert(other_id, message);
            } else {
                self.notifications.push(message);
            }
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn real_acp_process_frames_session_lifecycle_over_stdio() {
    let mut harness = Harness::start();
    harness.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false
            },
            "clientInfo": {"name": "acp-stdio-test", "version": "1"}
        }
    }));
    let initialized = harness.response(1);
    assert_eq!(initialized["result"]["protocolVersion"], 1);
    assert_eq!(initialized["result"]["agentInfo"]["name"], "lazar");

    harness.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "cwd": harness.root.to_string_lossy(),
            "mcpServers": [],
            "additionalDirectories": []
        }
    }));
    let created = harness.response(2);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session/new returns an id")
        .to_string();
    assert!(!session_id.is_empty());

    harness.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/list", "params": {}
    }));
    let listed = harness.response(3);
    assert!(listed["result"]["sessions"]
        .as_array()
        .expect("sessions array")
        .iter()
        .any(|row| row["sessionId"].as_str() == Some(session_id.as_str())));

    // Two newline-delimited requests are written back-to-back to exercise the
    // real server's framing loop rather than an in-process handler.
    let close = json!({
        "jsonrpc": "2.0", "id": 4, "method": "session/close",
        "params": {"sessionId": session_id}
    });
    let list_again = json!({
        "jsonrpc": "2.0", "id": 5, "method": "session/list", "params": {}
    });
    serde_json::to_writer(harness.stdin.as_mut().unwrap(), &close).unwrap();
    harness.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
    serde_json::to_writer(harness.stdin.as_mut().unwrap(), &list_again).unwrap();
    harness.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
    harness.stdin.as_mut().unwrap().flush().unwrap();
    harness.response(4);
    let after_close = harness.response(5);
    assert!(!after_close["result"]["sessions"]
        .as_array()
        .expect("sessions array")
        .iter()
        .any(|row| row["sessionId"].as_str() == Some(session_id.as_str())));
}

fn begin_active_hook(harness: &mut Harness) -> String {
    harness.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}));
    harness.response(1);
    harness.send(json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":harness.root,"mcpServers":[]}}));
    let id = harness.response(2)["result"]["sessionId"].as_str().unwrap().to_string();
    harness.send(json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":id,"prompt":[{"type":"text","text":"test cancellation"}]}}));
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !harness.root.join("active-pid").exists() {
        assert!(std::time::Instant::now() < deadline, "hook did not start");
        thread::sleep(Duration::from_millis(10));
    }
    id
}

#[test]
fn cancellation_waits_for_running_hook_and_close_fires_session_end() {
    let mut harness = Harness::start_with_hook(true);
    let id = begin_active_hook(&mut harness);
    harness.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":id}}));
    assert_eq!(harness.response(3)["result"]["stopReason"], "cancelled");
    harness.send(json!({"jsonrpc":"2.0","id":4,"method":"session/close","params":{"sessionId":id}}));
    harness.response(4);
    assert!(harness.root.join("closed").exists());
}

#[test]
fn deleting_active_session_does_not_recreate_its_log() {
    let mut harness = Harness::start_with_hook(true);
    let id = begin_active_hook(&mut harness);
    harness.send(json!({"jsonrpc":"2.0","id":4,"method":"session/delete","params":{"sessionId":id}}));
    harness.response(4);
    thread::sleep(Duration::from_millis(100));
    assert!(!harness.root.join("user/logs/sessions").join(format!("{id}.jsonl")).exists());
    assert!(harness.root.join("closed").exists());
}

#[test]
fn eof_cancels_running_hook_and_runs_shutdown_cleanup() {
    let mut harness = Harness::start_with_hook(true);
    begin_active_hook(&mut harness);
    harness.stdin.take();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while harness.child.try_wait().unwrap().is_none() {
        assert!(std::time::Instant::now() < deadline, "ACP failed to shut down");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(harness.root.join("closed").exists());
}

struct GroupCleanup(i32);
impl Drop for GroupCleanup {
    fn drop(&mut self) { unsafe { libc::kill(-self.0, libc::SIGKILL); } }
}

fn assert_process_stopped(pid: i32) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(std::time::Instant::now() < deadline, "process {pid} survived owner cleanup");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn cancellation_cleans_nested_kernel_process_groups() {
    use std::os::unix::fs::PermissionsExt;
    let mut harness = Harness::start_with_hook(true);
    let nested = harness.root.join("nested");
    fs::create_dir_all(nested.join("hooks/session-start.d")).unwrap();
    let hook = nested.join("hooks/session-start.d/active.sh");
    fs::write(&hook, "#!/bin/bash\necho $$ > \"$LAZAR_HOME/active-pid\"\nsleep 60 &\nwait\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(harness.root.join("hooks/user-prompt.d/active.sh"), format!(
        "#!/bin/bash\necho $$ > \"$LAZAR_HOME/active-pid\"\nLAZAR_HOME=\"$LAZAR_HOME/nested\" ANTHROPIC_API_KEY=offline-test-key LAZAR_NO_SANDBOX=1 \"{}\" -p nested\n",
        env!("CARGO_BIN_EXE_lazar")
    )).unwrap();
    let id = begin_active_hook(&mut harness);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !nested.join("active-pid").exists() {
        assert!(std::time::Instant::now() < deadline, "nested hook did not start");
        thread::sleep(Duration::from_millis(10));
    }
    let pid = fs::read_to_string(nested.join("active-pid")).unwrap().trim().parse::<i32>().unwrap();
    let _cleanup = GroupCleanup(pid);
    harness.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":id}}));
    assert_eq!(harness.response(3)["result"]["stopReason"], "cancelled");
    assert_process_stopped(pid);
}

#[test]
fn abrupt_kernel_exit_cleans_running_command_group() {
    let mut harness = Harness::start_with_hook(true);
    begin_active_hook(&mut harness);
    let pid = fs::read_to_string(harness.root.join("active-pid")).unwrap().trim().parse::<i32>().unwrap();
    let _cleanup = GroupCleanup(pid);
    harness.child.kill().unwrap();
    harness.child.wait().unwrap();
    assert_process_stopped(pid);
}

#[test]
fn session_listing_retains_project_cwd_after_process_restart() {
    let mut first = Harness::start();
    let project = first.root.join("original-project");
    fs::create_dir_all(&project).unwrap();
    first.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}));
    first.response(1);
    first.send(json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":project,"mcpServers":[]}}));
    let id = first.response(2)["result"]["sessionId"].as_str().unwrap().to_string();
    let log = first.root.join("user/logs/sessions").join(format!("{id}.jsonl"));
    fs::write(log, json!({"kind":"user","content":"persisted task"}).to_string() + "\n").unwrap();
    first.stdin.take();
    assert!(first.child.wait().unwrap().success());

    let mut second = Harness::start_at(first.root.clone(), false);
    second.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}));
    second.response(1);
    second.send(json!({"jsonrpc":"2.0","id":2,"method":"session/list","params":{"cwd":project}}));
    let listed = second.response(2);
    let row = listed["result"]["sessions"].as_array().unwrap().iter().find(|row| row["sessionId"] == id).unwrap();
    assert_eq!(row["cwd"], project.to_string_lossy().as_ref());
}

#[test]
fn failed_tool_replay_preserves_exit_code_on_the_acp_wire() {
    let mut harness = Harness::start();
    fs::create_dir_all(harness.root.join("user/logs/sessions")).unwrap();
    let events = [
        json!({"kind":"user","content":"run a command"}),
        json!({"kind":"assistant","content":[{"type":"tool_use","id":"one","name":"bash","input":{"command":"exit 23"}}]}),
        json!({"kind":"tool_result","tool_use_id":"one","content":"output rewritten by hook","exit_code":23}),
    ];
    fs::write(harness.root.join("user/logs/sessions/replay.jsonl"), events.iter().map(|event| format!("{event}\n")).collect::<String>()).unwrap();
    harness.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}));
    harness.response(1);
    harness.send(json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"replay","cwd":harness.root,"mcpServers":[]}}));
    harness.response(2);
    let update = harness.notifications.iter().map(|event| &event["params"]["update"])
        .find(|update| update["sessionUpdate"] == "tool_call_update").expect("replayed tool result");
    assert_eq!(update["status"], "failed");
    assert_eq!(update["rawOutput"]["exit_code"], 23);
}

#[test]
fn oversized_last_turn_reports_context_limit_instead_of_starting_fresh() {
    let mut harness = Harness::start();
    fs::create_dir_all(harness.root.join("user/logs/sessions")).unwrap();
    let mut log = json!({"kind":"user","content":"Keep the original task"}).to_string() + "\n";
    for index in 0..24 {
        log.push_str(&format!("{}\n", json!({"kind":"assistant","content":[
            {"type":"text","text":"x".repeat(2000)},
            {"type":"tool_use","id":format!("tool-{index}"),"name":"bash","input":{"command":"printf done"}}
        ]})));
        log.push_str(&format!("{}\n", json!({"kind":"tool_result","tool_use_id":format!("tool-{index}"),"content":"done","exit_code":0})));
    }
    fs::write(harness.root.join("user/logs/sessions/long.jsonl"), log).unwrap();
    harness.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}));
    harness.response(1);
    harness.send(json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"long","cwd":harness.root,"mcpServers":[]}}));
    harness.response(2);
    let replayed_rejection = harness.notifications.iter().any(|event| {
        let update = &event["params"]["update"];
        matches!(
            update["sessionUpdate"].as_str(),
            Some("user_message_chunk" | "agent_message_chunk")
        ) && update["content"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("session context limit"))
    });
    assert!(
        !replayed_rejection,
        "unreplayable history must not be emitted as a user or assistant turn: {:?}",
        harness.notifications
    );
    harness.notifications.clear();
    harness.send(json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"long","prompt":[{"type":"text","text":"continue"}]}}));
    let prompt = harness.response(3);
    let visible = harness.notifications.iter().filter_map(|event| event["params"]["update"]["content"]["text"].as_str()).collect::<Vec<_>>().join("\n");
    assert!(
        !visible.contains("session context limit"),
        "a long session must keep its latest turn instead of refusing: {prompt} {visible}"
    );
}
