//! Shared test helpers for agent-loop and provider integration tests.

#![allow(clippy::pedantic, dead_code, unused_imports)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use async_trait::async_trait;
use nca_common::config::{ModelPricing, ModelRetryConfig, NcaConfig, WebConfig};
use nca_common::event::AgentEvent;
use nca_common::message::Message;
use nca_common::tool::ToolDefinition;
use nca_core::agent::AgentLoop;
use nca_core::approval::ApprovalPolicy;
use nca_core::provider::{Provider, ProviderError, StreamChunk};
use nca_core::tools::ToolRegistry;
use tokio::sync::mpsc;

/// Build a readonly tool registry rooted at the current directory.
pub fn readonly_tools() -> ToolRegistry {
    ToolRegistry::with_default_readonly_tools(PathBuf::from("."), WebConfig::default())
}

/// Construct an [`AgentLoop`] wired for integration tests.
pub fn test_agent(
    provider: Box<dyn Provider>,
    tools: ToolRegistry,
    retry: ModelRetryConfig,
) -> (AgentLoop, mpsc::Receiver<AgentEvent>) {
    let (event_tx, event_rx) = mpsc::channel(64);
    let config = NcaConfig::default();
    let agent = AgentLoop::new(
        provider,
        tools,
        ApprovalPolicy::new(config.permissions.clone()),
        "test-model".into(),
        event_tx,
        8,
        8,
        0,
        None,
        ModelPricing::default(),
        retry,
    );
    (agent, event_rx)
}

/// Script of chunks returned by [`ScriptedProvider`].
#[derive(Debug, Clone)]
pub enum ProviderScriptStep {
    Chunks(Vec<StreamChunk>),
    Delay(std::time::Duration),
}

/// In-process provider that replays scripted stream chunks for deterministic tests.
pub struct ScriptedProvider {
    steps: Arc<Mutex<Vec<ProviderScriptStep>>>,
}

impl ScriptedProvider {
    pub fn new(steps: Vec<ProviderScriptStep>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps)),
        }
    }

    fn next_script(&self) -> Result<Vec<ProviderScriptStep>, ProviderError> {
        let mut guard = self
            .steps
            .lock()
            .map_err(|_| ProviderError::Other("script lock poisoned".into()))?;
        if guard.is_empty() {
            return Ok(vec![ProviderScriptStep::Chunks(vec![StreamChunk::Done])]);
        }
        Ok(vec![guard.remove(0)])
    }

    pub fn single_text(text: impl Into<String>) -> Self {
        Self::new(vec![ProviderScriptStep::Chunks(vec![
            StreamChunk::TextDelta(text.into()),
            StreamChunk::Usage {
                input_tokens: 1,
                output_tokens: 1,
            },
            StreamChunk::Done,
        ])])
    }

    pub fn slow_text(text: impl Into<String>, delay: std::time::Duration) -> Self {
        Self::new(vec![
            ProviderScriptStep::Delay(delay),
            ProviderScriptStep::Chunks(vec![
                StreamChunk::TextDelta(text.into()),
                StreamChunk::Done,
            ]),
        ])
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _workspace_root: &Path,
    ) -> Result<mpsc::Receiver<StreamChunk>, ProviderError> {
        let steps = self.next_script()?;

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for step in steps {
                match step {
                    ProviderScriptStep::Delay(duration) => {
                        tokio::time::sleep(duration).await;
                    }
                    ProviderScriptStep::Chunks(chunks) => {
                        for chunk in chunks {
                            let is_done = matches!(chunk, StreamChunk::Done);
                            if tx.send(chunk).await.is_err() {
                                return;
                            }
                            if is_done {
                                return;
                            }
                        }
                    }
                }
            }
            let _ = tx.send(StreamChunk::Done).await;
        });
        Ok(rx)
    }
}

/// Start a tiny_http SSE server for provider integration tests.
pub fn spawn_sse_server<F>(body: String, status: u16, assert_request: F) -> String
where
    F: FnOnce(&tiny_http::Request) + Send + 'static,
{
    use tiny_http::{Header, Response, Server, StatusCode};

    let server = Server::http("127.0.0.1:0").expect("start mock server");
    let base_url = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => format!("http://{addr}"),
        other => panic!("unsupported listen addr: {other:?}"),
    };

    thread::spawn(move || {
        let request = server.recv().expect("receive request");
        assert_request(&request);
        let response = Response::from_string(body)
            .with_status_code(StatusCode(status))
            .with_header(
                Header::from_bytes("Content-Type", "text/event-stream")
                    .expect("content type header"),
            );
        request.respond(response).expect("send response");
    });

    base_url
}

/// Drain a provider stream until `Done`.
pub async fn collect_chunks(mut rx: tokio::sync::mpsc::Receiver<StreamChunk>) -> Vec<StreamChunk> {
    let mut chunks = Vec::new();
    while let Some(chunk) = rx.recv().await {
        let done = matches!(chunk, StreamChunk::Done);
        chunks.push(chunk);
        if done {
            break;
        }
    }
    chunks
}
