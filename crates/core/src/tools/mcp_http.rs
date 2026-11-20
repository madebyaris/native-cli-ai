//! Streamable HTTP transport for MCP servers.
//!
//! Each JSON-RPC message is one POST to the server's endpoint. The reply is either a JSON body or
//! an SSE stream, and the server may issue an `Mcp-Session-Id` that later requests must echo.

use std::time::{Duration, Instant};

use nca_common::config::McpServerConfig;
use reqwest::StatusCode;
use reqwest::blocking::{Client, ClientBuilder, RequestBuilder, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use super::mcp::McpToolSchema;

const PROTOCOL_VERSION: &str = "2025-06-18";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Upper bound for one request, including a slow tool call.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ERROR_BODY_CHARS: usize = 300;

pub(super) struct McpHttpClient {
    server_name: String,
    url: String,
    client: Client,
    headers: HeaderMap,
    session_id: Option<String>,
    protocol_version: Option<String>,
    next_id: u64,
    deadline: Option<Instant>,
}

impl McpHttpClient {
    pub(super) fn connect(server: &McpServerConfig, url: &str) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        for (name, value) in &server.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|err| {
                format!(
                    "invalid header name `{name}` for MCP server `{}`: {err}",
                    server.name
                )
            })?;
            let header_value = HeaderValue::from_str(value).map_err(|err| {
                format!(
                    "invalid value for header `{name}` on MCP server `{}`: {err}",
                    server.name
                )
            })?;
            headers.insert(header_name, header_value);
        }
        if let Some(env_name) = &server.bearer_token_env {
            let token = std::env::var(env_name).map_err(|_| {
                format!(
                    "MCP server `{}` needs a bearer token, but environment variable `{env_name}` is not set",
                    server.name
                )
            })?;
            let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                format!("the token in `{env_name}` is not a valid HTTP header value")
            })?;
            headers.insert(AUTHORIZATION, value);
        }
        Self::with_client(&server.name, url, headers, Client::builder(), None)
    }

    pub(super) fn connect_anonymous(
        server_name: &str,
        url: &str,
        timeout: Duration,
    ) -> Result<Self, String> {
        let builder = Client::builder()
            .user_agent(concat!("nca/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none());
        Self::with_client(
            server_name,
            url,
            HeaderMap::new(),
            builder,
            Instant::now().checked_add(timeout),
        )
    }

    fn with_client(
        server_name: &str,
        url: &str,
        headers: HeaderMap,
        builder: ClientBuilder,
        deadline: Option<Instant>,
    ) -> Result<Self, String> {
        let client = builder
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|err| {
                format!("failed to build HTTP client for MCP server `{server_name}`: {err}")
            })?;
        Ok(Self {
            server_name: server_name.to_string(),
            url: url.to_string(),
            client,
            headers,
            session_id: None,
            protocol_version: None,
            next_id: 1,
            deadline,
        })
    }

    pub(super) fn initialize(&mut self) -> Result<(), String> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "nca", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        // The server answers with the version it speaks. Later requests must send that version.
        self.protocol_version = Some(
            result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION)
                .to_string(),
        );
        self.notify(
            "notifications/initialized",
            Value::Object(Default::default()),
        )
    }

    pub(super) fn list_tools(&mut self) -> Result<Vec<McpToolSchema>, String> {
        let result = self.request("tools/list", json!({}))?;
        serde_json::from_value(
            result
                .get("tools")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![])),
        )
        .map_err(|err| format!("failed to decode MCP tool list: {err}"))
    }

    pub(super) fn call_tool(&mut self, tool_name: &str, input: Value) -> Result<Value, String> {
        self.request(
            "tools/call",
            json!({ "name": tool_name, "arguments": input }),
        )
    }

    /// Ends the session when the server issued one. Servers may ignore this.
    pub(super) fn shutdown(&mut self) {
        let Some(session) = self.session_id.clone() else {
            return;
        };
        let mut request = self
            .client
            .delete(&self.url)
            .headers(self.headers.clone())
            .header("Mcp-Session-Id", session);
        if let Some(version) = &self.protocol_version {
            request = request.header("MCP-Protocol-Version", version);
        }
        let _ = self.send_request(request);
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let response = self.send(&body)?;
        self.remember_session(&response);
        if !response.status().is_success() {
            return Err(self.http_error(response));
        }
        let message = read_reply(response, id)?;
        if let Some(error) = message.get("error") {
            return Err(format!("MCP error: {error}"));
        }
        Ok(message
            .get("result")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default())))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let response = self.send(&body)?;
        self.remember_session(&response);
        if response.status().is_success() {
            Ok(())
        } else {
            Err(self.http_error(response))
        }
    }

    fn send(&self, body: &Value) -> Result<Response, String> {
        let mut request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .json(body);
        if let Some(version) = &self.protocol_version {
            request = request.header("MCP-Protocol-Version", version);
        }
        if let Some(session) = &self.session_id {
            request = request.header("Mcp-Session-Id", session);
        }
        self.send_request(request)
    }

    fn send_request(&self, mut request: RequestBuilder) -> Result<Response, String> {
        if let Some(deadline) = self.deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "MCP server `{}` search timed out",
                    self.server_name
                ));
            }
            request = request.timeout(remaining);
        }
        request
            .send()
            .map_err(|err| format!("MCP server `{}` request failed: {err}", self.server_name))
    }

    fn remember_session(&mut self, response: &Response) {
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            self.session_id = Some(session.to_string());
        }
    }

    fn http_error(&self, response: Response) -> String {
        let status = response.status();
        let body: String = response
            .text()
            .unwrap_or_default()
            .chars()
            .take(MAX_ERROR_BODY_CHARS)
            .collect();
        let hint = if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            " (check the token in the environment variable named by `bearer_token_env`)"
        } else {
            ""
        };
        format!(
            "MCP server `{}` returned HTTP {status}{hint}: {body}",
            self.server_name
        )
    }
}

/// Reads the reply to request `id` from either a JSON body or an SSE stream.
fn read_reply(response: Response, id: u64) -> Result<Value, String> {
    let is_sse = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let text = response
        .text()
        .map_err(|err| format!("failed to read MCP reply: {err}"))?;
    let messages = if is_sse {
        sse_messages(&text)
    } else {
        json_messages(&text)?
    };
    messages
        .into_iter()
        .find(|message| message.get("id") == Some(&Value::from(id)))
        .ok_or_else(|| format!("MCP server sent no reply to request {id}"))
}

fn json_messages(text: &str) -> Result<Vec<Value>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|err| format!("invalid MCP reply: {err}"))?;
    Ok(match value {
        Value::Array(items) => items,
        single => vec![single],
    })
}

/// SSE events carry JSON-RPC messages in `data:` lines. A blank line ends each event.
fn sse_messages(text: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut data = String::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Ok(message) = serde_json::from_str::<Value>(&data) {
                messages.push(message);
            }
            data.clear();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::load_mcp_tools;
    use nca_common::tool::ToolCall;
    use std::collections::BTreeMap;

    const TEST_SESSION: &str = "test-session";
    const TEST_TOKEN: &str = "Bearer test-token";

    fn header(name: &str, value: &str) -> tiny_http::Header {
        tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
    }

    fn request_header(request: &tiny_http::Request, name: &'static str) -> Option<String> {
        request
            .headers()
            .iter()
            .find(|candidate| candidate.field.equiv(name))
            .map(|candidate| candidate.value.as_str().to_string())
    }

    fn json_reply(id: &Value, result: Value) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
        let body = json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
        tiny_http::Response::from_string(body)
            .with_header(header("Content-Type", "application/json"))
    }

    fn sse_reply(id: &Value, result: Value) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
        let message = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        tiny_http::Response::from_string(format!("event: message\ndata: {message}\n\n"))
            .with_header(header("Content-Type", "text/event-stream"))
    }

    /// A streamable HTTP MCP server with one tool, `echo`. It requires the bearer token, and after
    /// `initialize` it requires the session and protocol-version headers.
    fn spawn_mcp_server() -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("start test MCP server");
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            other => panic!("unsupported listen addr: {other:?}"),
        };
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                if request_header(&request, "Authorization").as_deref() != Some(TEST_TOKEN) {
                    let _ = request.respond(
                        tiny_http::Response::from_string("unauthorized").with_status_code(401),
                    );
                    continue;
                }
                if request.method() == &tiny_http::Method::Delete {
                    let _ =
                        request.respond(tiny_http::Response::from_string("").with_status_code(200));
                    continue;
                }
                let session = request_header(&request, "Mcp-Session-Id");
                let protocol = request_header(&request, "MCP-Protocol-Version");
                let mut body = String::new();
                request
                    .as_reader()
                    .read_to_string(&mut body)
                    .expect("read request body");
                let message: Value = serde_json::from_str(&body).expect("JSON-RPC body");
                let method = message["method"].as_str().unwrap_or_default().to_string();
                let id = message["id"].clone();
                if method != "initialize"
                    && (session.as_deref() != Some(TEST_SESSION)
                        || protocol.as_deref() != Some(PROTOCOL_VERSION))
                {
                    let _ = request.respond(
                        tiny_http::Response::from_string("missing session or protocol header")
                            .with_status_code(400),
                    );
                    continue;
                }
                let response = match method.as_str() {
                    "initialize" => json_reply(
                        &id,
                        json!({
                            "protocolVersion": PROTOCOL_VERSION,
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "test", "version": "0" },
                        }),
                    )
                    .with_header(header("Mcp-Session-Id", TEST_SESSION)),
                    "notifications/initialized" => {
                        tiny_http::Response::from_string("").with_status_code(202)
                    }
                    "tools/list" => json_reply(
                        &id,
                        json!({ "tools": [{
                            "name": "echo",
                            "description": "Echo a word",
                            "inputSchema": {
                                "type": "object",
                                "properties": { "text": { "type": "string" } },
                                "required": ["text"],
                            },
                        }] }),
                    ),
                    "tools/call" => sse_reply(
                        &id,
                        json!({ "content": [{ "type": "text", "text": "pong" }] }),
                    ),
                    _ => json_reply(&id, json!({})),
                };
                let _ = request.respond(response);
            }
        });
        format!("http://127.0.0.1:{port}/mcp")
    }

    fn remote_server(url: String, headers: BTreeMap<String, String>) -> McpServerConfig {
        McpServerConfig {
            name: "remote".into(),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: Some(url),
            headers,
            bearer_token_env: None,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn discovers_and_calls_a_streamable_http_tool() {
        let url = spawn_mcp_server();
        let headers = BTreeMap::from([("Authorization".to_string(), TEST_TOKEN.to_string())]);
        let config = remote_server(url, headers);
        let root = std::env::temp_dir();
        // Blocking HTTP runs off the async runtime, the same way the supervisor calls it.
        let tools = tokio::task::spawn_blocking(move || load_mcp_tools(&root, &[config]))
            .await
            .expect("discovery task")
            .expect("discover tools over HTTP");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].definition().name, "mcp__remote__echo");

        let result = tools[0]
            .execute(&ToolCall {
                id: "call-1".into(),
                name: "mcp__remote__echo".into(),
                input: json!({ "text": "ping" }),
            })
            .await;
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("pong"), "{}", result.output);
    }

    #[test]
    fn missing_bearer_env_var_is_named_without_printing_any_secret() {
        let config = McpServerConfig {
            name: "remote".into(),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: Some("http://127.0.0.1:9/mcp".into()),
            headers: BTreeMap::new(),
            bearer_token_env: Some("NCA_TEST_UNSET_MCP_TOKEN".into()),
            enabled: true,
        };
        let error = McpHttpClient::connect(&config, "http://127.0.0.1:9/mcp")
            .err()
            .expect("a missing token must fail");
        assert!(error.contains("NCA_TEST_UNSET_MCP_TOKEN"), "{error}");
    }

    #[test]
    fn sse_reply_yields_the_message_for_the_request_id() {
        let text = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n\n";
        let messages = sse_messages(text);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["id"], json!(7));
    }
}
