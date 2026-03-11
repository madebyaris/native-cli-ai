use nca_common::event::AgentEvent;
use nca_common::message::{Message, MessageToolCall};
use nca_common::tool::{PermissionTier, ToolCall, ToolDefinition};

use crate::approval::ApprovalPolicy;
use crate::cost::CostTracker;
use crate::provider::{Provider, ProviderError, StreamChunk};
use crate::tools::ToolRegistry;

/// Drives the multi-turn conversation and tool-use loop.
pub struct AgentLoop {
    pub provider: Box<dyn Provider>,
    pub tools: ToolRegistry,
    pub approval: ApprovalPolicy,
    pub messages: Vec<Message>,
    pub model: String,
    pub cost_tracker: CostTracker,
    event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    max_turns: u32,
    max_tool_calls_per_turn: u32,
    checkpoint_interval: u32,
}

impl AgentLoop {
    pub fn new(
        provider: Box<dyn Provider>,
        tools: ToolRegistry,
        approval: ApprovalPolicy,
        model: String,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
        max_turns: u32,
        max_tool_calls_per_turn: u32,
        checkpoint_interval: u32,
    ) -> Self {
        Self {
            provider,
            tools,
            approval,
            messages: Vec::new(),
            model,
            cost_tracker: CostTracker::default(),
            event_tx,
            max_turns,
            max_tool_calls_per_turn,
            checkpoint_interval,
        }
    }

    /// Add a system prompt once at startup.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.messages.push(Message::system(prompt));
    }

    /// Run one turn: send messages to the provider, execute any tool calls,
    /// and repeat until the provider returns a final text response.
    pub async fn run_turn(&mut self, user_input: &str) -> Result<String, ProviderError> {
        self.messages.push(Message::user(user_input));
        self.emit(AgentEvent::MessageReceived {
            role: "user".into(),
            content: user_input.into(),
        })
        .await;

        let mut turn = 0_u32;
        let final_text = loop {
            turn += 1;
            if turn > self.max_turns {
                return Err(ProviderError::Other(format!(
                    "turn budget exceeded (max {})",
                    self.max_turns
                )));
            }

            self.emit(AgentEvent::Checkpoint {
                phase: "provider_request".into(),
                detail: format!("Starting model turn {turn}"),
                turn,
            })
            .await;
            let mut stream = self
                .provider
                .chat(&self.messages, &self.tool_definitions(), &self.model)
                .await?;

            let mut assistant_text = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();

            while let Some(chunk) = stream.recv().await {
                match chunk {
                    StreamChunk::TextDelta(delta) => {
                        assistant_text.push_str(&delta);
                        self.emit(AgentEvent::TokensStreamed { delta }).await;
                    }
                    StreamChunk::ToolUse(call) => {
                        self.emit(AgentEvent::ToolCallStarted {
                            call_id: call.id.clone(),
                            tool: call.name.clone(),
                            input: call.input.clone(),
                        })
                        .await;
                        tool_calls.push(call);
                    }
                    StreamChunk::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        self.cost_tracker.add(input_tokens, output_tokens);
                        self.emit(AgentEvent::CostUpdated {
                            input_tokens: self.cost_tracker.input_tokens,
                            output_tokens: self.cost_tracker.output_tokens,
                            estimated_cost_usd: self.cost_tracker.estimated_cost_usd(),
                        })
                        .await;
                    }
                    StreamChunk::Done => break,
                }
            }

            if tool_calls.is_empty() {
                self.messages.push(Message::assistant(assistant_text.clone()));
                self.emit(AgentEvent::MessageReceived {
                    role: "assistant".into(),
                    content: assistant_text.clone(),
                })
                .await;
                break assistant_text;
            }

            let replay_tool_calls = tool_calls
                .iter()
                .map(|call| MessageToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.input.clone(),
                })
                .collect();

            self.messages
                .push(Message::assistant_with_tool_calls(assistant_text, replay_tool_calls));

            if tool_calls.len() as u32 > self.max_tool_calls_per_turn {
                return Err(ProviderError::Other(format!(
                    "tool-call budget exceeded in turn {turn} ({} > {})",
                    tool_calls.len(),
                    self.max_tool_calls_per_turn
                )));
            }

            for (index, call) in tool_calls.into_iter().enumerate() {
                if self.checkpoint_interval > 0
                    && (index as u32 + 1) % self.checkpoint_interval == 0
                {
                    self.emit(AgentEvent::Checkpoint {
                        phase: "tool_execution".into(),
                        detail: format!("Executed {} tool calls in turn {turn}", index + 1),
                        turn,
                    })
                    .await;
                }
                let tier = self.approval.check(&call.name, &call.input.to_string());

                if tier == PermissionTier::Denied {
                    let result = nca_common::tool::ToolResult {
                        call_id: call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(format!("tool `{}` denied by policy", call.name)),
                    };
                    self.messages.push(Message::tool(
                        call.id.clone(),
                        format_tool_result(&result),
                    ));
                    self.emit(AgentEvent::ToolCallCompleted {
                        call_id: result.call_id.clone(),
                        output: result,
                    })
                    .await;
                    continue;
                }

                if tier == PermissionTier::Ask {
                    let description = format!("Tool `{}` requires approval", call.name);
                    self.emit(AgentEvent::ApprovalRequested {
                        call_id: call.id.clone(),
                        tool: call.name.clone(),
                        description: description.clone(),
                    })
                    .await;

                    let approved = self.approval.resolve(&call, &description).await;
                    self.emit(AgentEvent::ApprovalResolved {
                        call_id: call.id.clone(),
                        approved,
                    })
                    .await;

                    if !approved {
                        let result = nca_common::tool::ToolResult {
                            call_id: call.id.clone(),
                            success: false,
                            output: String::new(),
                            error: Some(format!(
                                "tool `{}` requires approval; request was denied",
                                call.name
                            )),
                        };
                        self.messages.push(Message::tool(
                            call.id.clone(),
                            format_tool_result(&result),
                        ));
                        self.emit(AgentEvent::ToolCallCompleted {
                            call_id: result.call_id.clone(),
                            output: result,
                        })
                        .await;
                        continue;
                    }

                    let result = self.tools.execute(&call).await;
                    self.messages
                        .push(Message::tool(call.id.clone(), format_tool_result(&result)));
                    self.emit(AgentEvent::ToolCallCompleted {
                        call_id: result.call_id.clone(),
                        output: result,
                    })
                    .await;
                    continue;
                }

                let result = self.tools.execute(&call).await;
                self.messages
                    .push(Message::tool(call.id.clone(), format_tool_result(&result)));
                self.emit(AgentEvent::ToolCallCompleted {
                    call_id: result.call_id.clone(),
                    output: result,
                })
                .await;
            }
        };

        if self.cost_tracker.input_tokens == 0 && self.cost_tracker.output_tokens == 0 {
            let estimated_input = (self
                .messages
                .iter()
                .map(|message| message.content.len())
                .sum::<usize>()
                / 4) as u64;
            let estimated_output = (final_text.len() / 4) as u64;
            self.cost_tracker.add(estimated_input, estimated_output);
            self.emit(AgentEvent::CostUpdated {
                input_tokens: self.cost_tracker.input_tokens,
                output_tokens: self.cost_tracker.output_tokens,
                estimated_cost_usd: self.cost_tracker.estimated_cost_usd(),
            })
            .await;
        }

        Ok(final_text)
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.definitions()
    }

    async fn emit(&self, event: AgentEvent) {
        let _ = self.event_tx.send(event).await;
    }

    pub fn event_sender(&self) -> Option<tokio::sync::mpsc::Sender<AgentEvent>> {
        Some(self.event_tx.clone())
    }
}

fn format_tool_result(result: &nca_common::tool::ToolResult) -> String {
    if result.success {
        result.output.clone()
    } else {
        result
            .error
            .clone()
            .unwrap_or_else(|| "tool failed".to_string())
    }
}
