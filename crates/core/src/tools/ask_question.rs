//! Interactive multiple-choice / custom / suggested-answer questions for the user.

use nca_common::event::{
    AgentEvent, InteractiveQuestionPayload, QuestionOption, QuestionSelection,
};
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use super::ToolExecutor;

fn selection_summary(sel: &QuestionSelection, payload: &InteractiveQuestionPayload) -> String {
    match sel {
        QuestionSelection::Suggested => {
            format!("Selected suggested answer: {}", payload.suggested_answer)
        }
        QuestionSelection::Option { option_id } => {
            let label = payload
                .options
                .iter()
                .find(|o| o.id == *option_id)
                .map(|o| o.label.as_str())
                .unwrap_or(option_id.as_str());
            format!("Selected option `{option_id}`: {label}")
        }
        QuestionSelection::Custom { text } => format!("Custom answer: {text}"),
    }
}

/// Tool that blocks until the user answers via CLI or `AgentCommand::AnswerQuestion`.
pub struct AskQuestionTool {
    event_tx: mpsc::Sender<AgentEvent>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<QuestionSelection>>>>,
}

impl AskQuestionTool {
    pub fn new(
        event_tx: mpsc::Sender<AgentEvent>,
        pending: Arc<Mutex<HashMap<String, oneshot::Sender<QuestionSelection>>>>,
    ) -> Self {
        Self { event_tx, pending }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for AskQuestionTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "ask_question".into(),
            description: "Ask the user a structured question with multiple choices, an optional \
                custom text answer, and a suggested default (always provide `suggested_answer`). \
                The UI shows options and the suggestion; the user can pick one, type custom text, \
                or accept the suggestion (e.g. `/auto-answer`). Use this instead of long numbered \
                lists in plain assistant text when you need a fast, reliable answer."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The question text shown to the user."
                    },
                    "options": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "Stable id for this option (e.g. static_site)." },
                                "label": { "type": "string", "description": "Human-readable label." }
                            },
                            "required": ["id", "label"]
                        },
                        "description": "List of choices; each needs a unique id."
                    },
                    "allow_custom": {
                        "type": "boolean",
                        "description": "If true, user can submit freeform text. Default true."
                    },
                    "suggested_answer": {
                        "type": "string",
                        "description": "Your best guess / recommendation; always set this so the user can accept quickly."
                    }
                },
                "required": ["prompt", "options", "suggested_answer"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let prompt = call.input["prompt"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        if prompt.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("prompt is required".into()),
            };
        }

        let suggested = call.input["suggested_answer"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        if suggested.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(
                    "suggested_answer is required (provide your recommended choice)".into(),
                ),
            };
        }

        let allow_custom = call
            .input
            .get("allow_custom")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let options: Vec<QuestionOption> = match call.input.get("options") {
            Some(serde_json::Value::Array(arr)) => {
                let mut out = Vec::new();
                for v in arr {
                    let id = v
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let label = v
                        .get("label")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if id.is_empty() || label.is_empty() {
                        return ToolResult {
                            call_id: call.id.clone(),
                            success: false,
                            output: String::new(),
                            error: Some("each option needs non-empty id and label".into()),
                        };
                    }
                    out.push(QuestionOption { id, label });
                }
                out
            }
            _ => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some("options must be a non-empty array of {id, label}".into()),
                };
            }
        };

        if options.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("at least one option is required".into()),
            };
        }

        let question_id = format!("q-{}", call.id);
        let payload = InteractiveQuestionPayload {
            question_id: question_id.clone(),
            call_id: call.id.clone(),
            prompt,
            options,
            allow_custom,
            suggested_answer: suggested,
        };

        let (tx, rx) = oneshot::channel();
        {
            let mut m = self.pending.lock().unwrap();
            m.insert(question_id.clone(), tx);
        }

        if self
            .event_tx
            .send(AgentEvent::QuestionRequested {
                question: payload.clone(),
            })
            .await
            .is_err()
        {
            let mut m = self.pending.lock().unwrap();
            m.remove(&question_id);
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("failed to emit QuestionRequested (session ended?)".into()),
            };
        }

        let selection = match rx.await {
            Ok(sel) => sel,
            Err(_) => {
                let mut m = self.pending.lock().unwrap();
                m.remove(&question_id);
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(
                        "channel closed waiting for question answer (session ended?)".into(),
                    ),
                };
            }
        };

        let summary = selection_summary(&selection, &payload);
        let _ = self
            .event_tx
            .send(AgentEvent::QuestionResolved {
                question_id: question_id.clone(),
                selection: selection.clone(),
            })
            .await;

        ToolResult {
            call_id: call.id.clone(),
            success: true,
            output: summary,
            error: None,
        }
    }
}
