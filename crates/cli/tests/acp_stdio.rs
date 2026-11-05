//! End-to-end tests for `nca acp`: a scripted ACP client over the real stdio pipes.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

fn write_workspace_config(workspace: &Path, base_url: &str) {
    write_workspace_config_with(workspace, base_url, "");
}

fn write_workspace_config_with(workspace: &Path, base_url: &str, extra: &str) {
    let dir = workspace.join(".nca");
    std::fs::create_dir_all(&dir).expect("create .nca");
    std::fs::write(
        dir.join("config.local.toml"),
        format!(
            "[provider]\ndefault = \"custom\"\n\n[provider.custom]\napi_key = \"test-key\"\nbase_url = \"{base_url}\"\nmodel = \"acp-test-model\"\ncompatibility = \"openai\"\n{extra}"
        ),
    )
    .expect("write workspace config");
}

struct AcpClient {
    child: Arc<Mutex<Child>>,
    stdin: Option<ChildStdin>,
    stdout: std::io::Lines<BufReader<ChildStdout>>,
    next_id: u64,
    notifications: Vec<Value>,
    permission_requests: Vec<Value>,
    /// Option id used to answer `session/request_permission`.
    permission_answer: &'static str,
}

impl AcpClient {
    fn spawn(workspace: &Path, home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nca"))
            .arg("acp")
            .current_dir(workspace)
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_RUNTIME_DIR", home.join("run"))
            // The mock providers serve chat completions only; skip the model
            // catalog request so it cannot take one of their connections.
            .env("NCA_SKIP_CONTEXT_API", "1")
            .env_remove("MINIMAX_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("CUSTOM_PROVIDER_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn nca acp");

        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout");
        let child = Arc::new(Mutex::new(child));

        let watchdog = child.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(30));
            if let Ok(mut child) = watchdog.try_lock() {
                let _ = child.kill();
            }
        });

        Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
            notifications: Vec::new(),
            permission_requests: Vec::new(),
            permission_answer: "allow_once",
        }
    }

    fn send(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        let mut line = serde_json::to_string(&value).expect("serialize");
        line.push('\n');
        stdin.write_all(line.as_bytes()).expect("write request");
        stdin.flush().expect("flush");
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.read_until_response(id)
    }

    fn request_error(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .stdout
                .next()
                .expect("agent closed stdout")
                .expect("read stdout line");
            let value: Value = serde_json::from_str(&line).expect("stdout must be JSON-RPC only");
            if self.maybe_answer_permission(&value) {
                continue;
            }
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value
                    .get("error")
                    .cloned()
                    .expect("expected a JSON-RPC error");
            }
            self.notifications.push(value);
        }
    }

    /// Answer `session/request_permission` like a client would, with
    /// `permission_answer` (allow once by default).
    fn maybe_answer_permission(&mut self, value: &Value) -> bool {
        if value.get("method").and_then(Value::as_str) != Some("session/request_permission") {
            return false;
        }
        self.permission_requests.push(value.clone());
        self.send(json!({
            "jsonrpc": "2.0",
            "id": value["id"].clone(),
            "result": {"outcome": {"outcome": "selected", "optionId": self.permission_answer}},
        }));
        true
    }

    fn read_until_response(&mut self, id: u64) -> Value {
        loop {
            let line = self
                .stdout
                .next()
                .expect("agent closed stdout")
                .expect("read stdout line");
            let value: Value = serde_json::from_str(&line).expect("stdout must be JSON-RPC only");
            if self.maybe_answer_permission(&value) {
                continue;
            }
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                assert!(value.get("error").is_none(), "unexpected error: {value}");
                return value.get("result").cloned().unwrap_or(Value::Null);
            }
            assert!(value.get("method").is_some(), "stray message: {value}");
            self.notifications.push(value);
        }
    }

    fn initialize(&mut self) -> Value {
        self.initialize_with(json!({}))
    }

    fn initialize_with(&mut self, client_capabilities: Value) -> Value {
        self.request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": client_capabilities}),
        )
    }

    fn new_session(&mut self, cwd: &Path) -> String {
        let result = self.request("session/new", json!({"cwd": cwd, "mcpServers": []}));
        result["sessionId"].as_str().expect("sessionId").to_string()
    }

    fn prompt(&mut self, session_id: &str, text: &str) -> Value {
        self.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": text}],
            }),
        )
    }

    fn updates(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.notifications)
            .into_iter()
            .filter_map(|message| {
                if message["method"] == "session/update" {
                    Some(message["params"]["update"].clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Read notifications until one `session/update` of `kind` arrives. Keeps
    /// the earlier updates so `updates()` can still inspect them.
    fn wait_for_update(&mut self, kind: &str) -> Value {
        let matches = |message: &Value| {
            message["method"] == "session/update"
                && message["params"]["update"]["sessionUpdate"] == kind
        };
        if let Some(index) = self.notifications.iter().position(matches) {
            return self.notifications.remove(index)["params"]["update"].clone();
        }
        loop {
            let line = self
                .stdout
                .next()
                .expect("agent closed stdout")
                .expect("read stdout line");
            let value: Value = serde_json::from_str(&line).expect("stdout must be JSON-RPC only");
            if self.maybe_answer_permission(&value) {
                continue;
            }
            if matches(&value) {
                return value["params"]["update"].clone();
            }
            self.notifications.push(value);
        }
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        self.stdin.take();
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn sse_body(text: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}},\"index\":0,\"finish_reason\":null}}]}}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":12,\"completion_tokens\":4}}}}\n\ndata: [DONE]\n\n"
    )
}

/// One SSE reply made of many small deltas, to exercise coalescing.
fn sse_body_chunks(chunks: &[&str]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str(&format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{chunk}\"}},\"index\":0,\"finish_reason\":null}}]}}\n\n"
        ));
    }
    body.push_str(
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4}}\n\ndata: [DONE]\n\n",
    );
    body
}

/// One-shot HTTP server for one ACP turn. It answers the runtime's
/// `GET /v1/models` context-window probe first, then serves each chat reply in
/// `responses` in order (one reply per provider request).
fn spawn_sse_provider(text: &str) -> String {
    spawn_scripted_provider(vec![sse_body(text)])
}

fn spawn_scripted_provider(mut responses: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind sse provider");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        loop {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(_) => return,
            };
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut content_length = 0usize;
            let mut path = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).expect("read head") == 0 {
                    break;
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if path.is_empty()
                    && let Some(rest) = line.strip_prefix("GET ")
                {
                    path = rest
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                }
                let lower = line.to_ascii_lowercase();
                if let Some(value) = lower.strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut request_body = vec![0u8; content_length];
            let _ = reader.read_exact(&mut request_body);

            let models = path.starts_with("/v1/models");
            let (content_type, payload) = if models {
                (
                    "application/json",
                    r#"{"data":[{"id":"acp-test-model","context_length":32000}]}"#.to_string(),
                )
            } else if responses.is_empty() {
                ("text/event-stream", sse_body(""))
            } else {
                ("text/event-stream", responses.remove(0))
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

/// Accept connections and never answer, so the turn stays in flight.
fn hanging_provider() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind hanging provider");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => held.push(stream),
                Err(_) => break,
            }
        }
    });
    format!("http://{addr}")
}

fn session_update_kinds(updates: &[Value]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|update| update["sessionUpdate"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn initialize_negotiates_v1_and_advertises_capabilities() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    write_workspace_config(workspace.path(), "http://127.0.0.1:1");
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    let result = client.initialize();
    assert_eq!(result["protocolVersion"], 1);
    assert_eq!(result["agentCapabilities"]["loadSession"], true);
    assert_eq!(
        result["agentCapabilities"]["promptCapabilities"]["image"],
        true
    );
    assert!(result["agentCapabilities"]["sessionCapabilities"]["list"].is_object());
    assert!(result["agentCapabilities"]["sessionCapabilities"]["delete"].is_object());
    assert_eq!(result["agentInfo"]["name"], "nca");
}

#[test]
fn new_session_exposes_modes_and_model_option() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    write_workspace_config(workspace.path(), "http://127.0.0.1:1");
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let result = client.request(
        "session/new",
        json!({"cwd": workspace.path(), "mcpServers": []}),
    );

    assert!(
        result["sessionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    let mode_ids: Vec<&str> = result["modes"]["availableModes"]
        .as_array()
        .expect("modes")
        .iter()
        .filter_map(|mode| mode["id"].as_str())
        .collect();
    assert!(mode_ids.contains(&"plan"));
    assert!(mode_ids.contains(&"ask"));
    assert_eq!(result["modes"]["currentModeId"], "ask");

    let model_option = result["configOptions"]
        .as_array()
        .expect("configOptions")
        .iter()
        .find(|option| option["category"] == "model")
        .expect("model option");
    assert_eq!(model_option["id"], "model");
    assert_eq!(model_option["currentValue"], "acp-test-model");
}

#[test]
fn prompt_streams_chunks_and_ends_the_turn() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    let base_url = spawn_sse_provider("Hello from ACP");
    write_workspace_config(workspace.path(), &base_url);
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let session_id = client.new_session(workspace.path());
    let result = client.prompt(&session_id, "say hello");

    assert_eq!(result["stopReason"], "end_turn");
    let updates = client.updates();
    let kinds = session_update_kinds(&updates);
    assert!(
        kinds.iter().any(|kind| kind == "agent_message_chunk"),
        "expected streamed text, got {kinds:?}"
    );
    let text: String = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["content"]["text"].as_str())
        .collect();
    assert!(text.contains("Hello from ACP"), "got {text:?}");
    assert!(
        kinds.iter().any(|kind| kind == "usage_update"),
        "expected a context usage update, got {kinds:?}"
    );
}

#[test]
fn streamed_chunks_are_coalesced_without_losing_text() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    let deltas = vec!["ab"; 200];
    let base_url = spawn_scripted_provider(vec![sse_body_chunks(&deltas)]);
    write_workspace_config(workspace.path(), &base_url);
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let session_id = client.new_session(workspace.path());
    assert_eq!(
        client.prompt(&session_id, "stream a lot")["stopReason"],
        "end_turn"
    );

    let updates = client.updates();
    let chunks: Vec<&str> = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["content"]["text"].as_str())
        .collect();
    let text: String = chunks.concat();

    assert_eq!(text, "ab".repeat(200), "coalescing must not drop text");
    assert!(
        chunks.len() < 200,
        "expected coalesced chunks, got {}",
        chunks.len()
    );
}

#[test]
fn cancel_returns_cancelled_and_keeps_the_session_usable() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    let base_url = hanging_provider();
    write_workspace_config(workspace.path(), &base_url);
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let session_id = client.new_session(workspace.path());

    let id = client.next_id;
    client.next_id += 1;
    client.send(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "this never answers"}],
        },
    }));
    std::thread::sleep(Duration::from_millis(300));
    client.notify("session/cancel", json!({"sessionId": session_id}));

    let result = client.read_until_response(id);
    assert_eq!(result["stopReason"], "cancelled");

    // A cancelled turn must not poison the session: the next prompt is accepted.
    let second = client.request(
        "session/set_mode",
        json!({"sessionId": session_id, "modeId": "plan"}),
    );
    assert!(second.is_object());
}

#[test]
fn load_session_replays_history_from_disk() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    let base_url = spawn_sse_provider("Remembered answer");
    write_workspace_config(workspace.path(), &base_url);

    let session_id = {
        let mut client = AcpClient::spawn(workspace.path(), home.path());
        client.initialize();
        let session_id = client.new_session(workspace.path());
        assert_eq!(
            client.prompt(&session_id, "remember this")["stopReason"],
            "end_turn"
        );
        session_id
    };

    let mut client = AcpClient::spawn(workspace.path(), home.path());
    client.initialize();
    let result = client.request(
        "session/load",
        json!({"sessionId": session_id, "cwd": workspace.path(), "mcpServers": []}),
    );
    assert!(result["modes"]["availableModes"].is_array());

    let updates = client.updates();
    let kinds = session_update_kinds(&updates);
    assert!(
        kinds.iter().any(|kind| kind == "user_message_chunk"),
        "expected replayed user message, got {kinds:?}"
    );
    let text: String = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["content"]["text"].as_str())
        .collect();
    assert!(text.contains("Remembered answer"), "got {text:?}");
}

#[test]
fn list_sessions_returns_the_workspace_sessions() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    let base_url = spawn_sse_provider("ok");
    write_workspace_config(workspace.path(), &base_url);

    let session_id = {
        let mut client = AcpClient::spawn(workspace.path(), home.path());
        client.initialize();
        let session_id = client.new_session(workspace.path());
        assert_eq!(client.prompt(&session_id, "hi")["stopReason"], "end_turn");
        session_id
    };

    let mut client = AcpClient::spawn(workspace.path(), home.path());
    client.initialize();
    let result = client.request("session/list", json!({"cwd": workspace.path()}));
    let sessions = result["sessions"].as_array().expect("sessions");
    assert!(
        sessions
            .iter()
            .any(|session| session["sessionId"] == session_id),
        "expected {session_id} in {sessions:?}"
    );
}

#[test]
fn missing_api_key_fails_loudly_without_reading_stdin() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    // No provider config at all: the default provider has no credentials.
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let error = client.request_error(
        "session/new",
        json!({"cwd": workspace.path(), "mcpServers": []}),
    );
    let message = error["data"]
        .as_str()
        .or_else(|| error["message"].as_str())
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        message.contains("api key"),
        "expected a loud credentials error, got {error}"
    );

    // The process must stay up and keep serving, not hang on stdin.
    let result = client.request("session/list", json!({"cwd": workspace.path()}));
    assert!(result["sessions"].is_array());
}

fn tool_call_body(id: &str, name: &str, arguments: Value) -> String {
    let delta = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": id,
                    "type": "function",
                    "function": {"name": name, "arguments": arguments.to_string()},
                }],
            },
            "index": 0,
            "finish_reason": null,
        }],
    });
    format!(
        "data: {delta}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n"
    )
}

#[test]
fn tool_calls_are_reported_with_kind_and_locations() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    std::fs::write(workspace.path().join("note.txt"), "hello note\n").expect("write note");
    let base_url = spawn_scripted_provider(vec![
        tool_call_body("call_1", "read_file", json!({"path": "note.txt"})),
        sse_body("done reading"),
    ]);
    write_workspace_config_with(
        workspace.path(),
        &base_url,
        "[permissions]\nmode = \"bypass-permissions\"\n",
    );
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let session_id = client.new_session(workspace.path());
    let result = client.prompt(&session_id, "read note.txt");
    assert_eq!(result["stopReason"], "end_turn");

    let updates = client.updates();
    let started = updates
        .iter()
        .find(|update| update["sessionUpdate"] == "tool_call")
        .expect("tool_call update");
    assert_eq!(started["toolCallId"], "call_1");
    assert_eq!(started["kind"], "read");
    assert_eq!(started["status"], "in_progress");
    let expected = std::fs::canonicalize(workspace.path())
        .expect("canonical workspace")
        .join("note.txt");
    assert_eq!(
        started["locations"][0]["path"],
        expected.to_str().expect("utf8 path")
    );

    let completed = updates
        .iter()
        .find(|update| update["sessionUpdate"] == "tool_call_update")
        .expect("tool_call_update");
    assert_eq!(completed["toolCallId"], "call_1");
    assert_eq!(completed["status"], "completed");
    let output = completed["content"][0]["content"]["text"]
        .as_str()
        .expect("tool content");
    assert!(output.contains("hello note"), "got {output:?}");

    let text: String = updates
        .iter()
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update["content"]["text"].as_str())
        .collect();
    assert!(text.contains("done reading"), "got {text:?}");
}

#[test]
fn permission_requests_are_answered_and_the_tool_runs() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    let base_url = spawn_scripted_provider(vec![
        tool_call_body(
            "call_bash",
            "execute_bash",
            json!({"command": "rm -f stale.txt; printf hi > out.txt"}),
        ),
        sse_body("ran the command"),
    ]);
    // Default permission mode asks before dangerous shell commands (`rm`).
    write_workspace_config(workspace.path(), &base_url);
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let session_id = client.new_session(workspace.path());
    let result = client.prompt(&session_id, "write a file");
    assert_eq!(result["stopReason"], "end_turn");

    assert_eq!(
        client.permission_requests.len(),
        1,
        "expected exactly one approval request"
    );
    let request = &client.permission_requests[0];
    assert_eq!(request["params"]["sessionId"], session_id);
    assert_eq!(request["params"]["toolCall"]["kind"], "execute");
    let options: Vec<&str> = request["params"]["options"]
        .as_array()
        .expect("options")
        .iter()
        .filter_map(|option| option["optionId"].as_str())
        .collect();
    assert!(options.contains(&"allow_once"));
    assert!(options.contains(&"reject_once"));

    let content = std::fs::read_to_string(workspace.path().join("out.txt")).expect("read output");
    assert_eq!(content, "hi");
}

#[test]
fn reject_for_this_session_is_not_asked_again() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    let command = json!({"command": "rm -f stale.txt; printf hi > out.txt"});
    let base_url = spawn_scripted_provider(vec![
        tool_call_body("call_1", "execute_bash", command.clone()),
        tool_call_body("call_2", "execute_bash", command),
        sse_body("gave up"),
    ]);
    write_workspace_config(workspace.path(), &base_url);
    let mut client = AcpClient::spawn(workspace.path(), home.path());
    client.permission_answer = "reject_always";

    client.initialize();
    let session_id = client.new_session(workspace.path());
    let result = client.prompt(&session_id, "write a file");
    assert_eq!(result["stopReason"], "end_turn");

    assert_eq!(
        client.permission_requests.len(),
        1,
        "the retried call must be denied without asking again"
    );
    assert!(!workspace.path().join("out.txt").exists());
}

#[test]
fn new_session_advertises_slash_commands() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    write_workspace_config(workspace.path(), "http://127.0.0.1:1");
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let _session_id = client.new_session(workspace.path());

    let update = client.wait_for_update("available_commands_update");
    let names: Vec<&str> = update["availableCommands"]
        .as_array()
        .expect("availableCommands")
        .iter()
        .filter_map(|command| command["name"].as_str())
        .collect();
    assert_eq!(names, ["compact", "rename", "model", "mode"]);
}

#[test]
fn rename_command_updates_the_session_title() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    write_workspace_config(workspace.path(), "http://127.0.0.1:1");
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let session_id = client.new_session(workspace.path());
    let result = client.prompt(&session_id, "/rename parser cleanup");
    assert_eq!(result["stopReason"], "end_turn");

    let info = client.wait_for_update("session_info_update");
    assert_eq!(info["title"], "parser cleanup");

    let listed = client.request("session/list", json!({"cwd": workspace.path()}));
    assert!(
        listed["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|session| session["sessionId"] == session_id
                && session["title"] == "parser cleanup"),
        "title must be persisted: {listed}"
    );
}

#[test]
fn compact_command_ends_the_turn_with_feedback() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    write_workspace_config(workspace.path(), "http://127.0.0.1:1");
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let session_id = client.new_session(workspace.path());
    let result = client.prompt(&session_id, "/compact");
    assert_eq!(result["stopReason"], "end_turn");

    let chunk = client.wait_for_update("agent_message_chunk");
    assert!(
        chunk["content"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Context compacted")),
        "got {chunk}"
    );
}

#[test]
fn delete_session_removes_it_from_the_list() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    write_workspace_config(workspace.path(), "http://127.0.0.1:1");
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize();
    let session_id = client.new_session(workspace.path());
    let result = client.request("session/delete", json!({"sessionId": session_id}));
    assert!(result.is_object(), "delete response: {result}");

    let listed = client.request("session/list", json!({"cwd": workspace.path()}));
    assert!(
        !listed["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|session| session["sessionId"] == session_id),
        "deleted session is still listed: {listed}"
    );

    let error = client.request_error(
        "session/load",
        json!({"sessionId": session_id, "cwd": workspace.path(), "mcpServers": []}),
    );
    let message = error["data"]
        .as_str()
        .or_else(|| error["message"].as_str())
        .unwrap_or_default();
    assert!(
        message.contains("cannot load session"),
        "expected a load error after delete, got {error}"
    );
}

#[test]
fn delete_session_works_in_a_fresh_process_without_listing() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    write_workspace_config(workspace.path(), "http://127.0.0.1:1");

    let session_id = {
        let mut first = AcpClient::spawn(workspace.path(), home.path());
        first.initialize();
        first.new_session(workspace.path())
    };

    let mut second = AcpClient::spawn(workspace.path(), home.path());
    second.initialize();
    let result = second.request("session/delete", json!({"sessionId": session_id}));
    assert!(result.is_object(), "delete response: {result}");

    let listed = second.request("session/list", json!({"cwd": workspace.path()}));
    assert!(
        !listed["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|session| session["sessionId"] == session_id),
        "deleted session is still listed: {listed}"
    );
}

#[test]
fn compaction_updates_are_sent_when_the_client_supports_them() {
    let workspace = TempDir::new().expect("workspace");
    let home = TempDir::new().expect("home");
    let base_url = spawn_scripted_provider(vec![sse_body("answer"), sse_body("summary text")]);
    write_workspace_config_with(
        workspace.path(),
        &base_url,
        "[memory.context]\ncontext_window_target = 100\nauto_detect_context_window = false\nmax_retained_messages = 1\nauto_summarize_threshold = 1\n",
    );
    let mut client = AcpClient::spawn(workspace.path(), home.path());

    client.initialize_with(json!({"session": {"compaction": {}}}));
    let session_id = client.new_session(workspace.path());
    let result = client.prompt(&session_id, "hello");
    assert_eq!(result["stopReason"], "end_turn");

    let mut statuses = Vec::new();
    for _ in 0..4 {
        let update = client.wait_for_update("compaction_update");
        statuses.push(update["status"].as_str().unwrap_or_default().to_string());
        if update["status"] == "completed" {
            assert!(
                update["summary"][0]["text"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "completed compaction must carry a summary: {update}"
            );
            break;
        }
    }
    assert!(
        statuses.iter().any(|status| status == "completed"),
        "expected a completed compaction, got {statuses:?}"
    );
}
