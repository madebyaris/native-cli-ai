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

const WAIT_SECS: u64 = 3600;

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

        let selection =
            match tokio::time::timeout(std::time::Duration::from_secs(WAIT_SECS), rx).await {
                Ok(Ok(sel)) => sel,
                Ok(Err(_)) | Err(_) => {
                    let mut m = self.pending.lock().unwrap();
                    m.remove(&question_id);
                    return ToolResult {
                        call_id: call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(
                            "timed out or channel closed waiting for question answer; use IPC \
                         AnswerQuestion or an interactive terminal"
                                .into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::tool::ToolCall;
    use std::time::Duration;

    fn sample_call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "ask_question".into(),
            input: serde_json::json!({
                "prompt": "Pick a stack",
                "suggested_answer": "vite",
                "options": [
                    {"id": "vite", "label": "Vanilla Vite"},
                    {"id": "r3f", "label": "React Three Fiber"}
                ]
            }),
        }
    }

    #[tokio::test]
    async fn happy_suggested_answer_unblocks_tool() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let tool = AskQuestionTool::new(event_tx, pending.clone());
        let call = sample_call("c1");
        let qid = format!("q-{}", call.id);

        let exec = tokio::spawn(async move { tool.execute(&call).await });
        let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .expect("timeout")
            .expect("event");
        match event {
            AgentEvent::QuestionRequested { question } => {
                assert_eq!(question.question_id, qid);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        {
            let mut m = pending.lock().unwrap();
            let tx = m.remove(&qid).expect("pending");
            tx.send(QuestionSelection::Suggested).unwrap();
        }

        let result = exec.await.expect("join");
        assert!(result.success);
        assert!(result.output.contains("Selected suggested answer: vite"));

        let resolved = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .expect("timeout")
            .expect("resolved");
        assert!(matches!(
            resolved,
            AgentEvent::QuestionResolved {
                selection: QuestionSelection::Suggested,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn happy_option_selection() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let tool = AskQuestionTool::new(event_tx, pending.clone());
        let call = sample_call("c-opt");
        let qid = format!("q-{}", call.id);
        let exec = tokio::spawn(async move { tool.execute(&call).await });
        let _ = event_rx.recv().await;
        {
            let mut m = pending.lock().unwrap();
            m.remove(&qid)
                .unwrap()
                .send(QuestionSelection::Option {
                    option_id: "r3f".into(),
                })
                .unwrap();
        }
        let result = exec.await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("`r3f`"));
        assert!(result.output.contains("React Three Fiber"));
    }

    #[tokio::test]
    async fn happy_custom_selection() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let tool = AskQuestionTool::new(event_tx, pending.clone());
        let call = sample_call("c-custom");
        let qid = format!("q-{}", call.id);
        let exec = tokio::spawn(async move { tool.execute(&call).await });
        let _ = event_rx.recv().await;
        {
            let mut m = pending.lock().unwrap();
            m.remove(&qid)
                .unwrap()
                .send(QuestionSelection::Custom {
                    text: "single html file".into(),
                })
                .unwrap();
        }
        let result = exec.await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Custom answer: single html file"));
    }

    #[tokio::test]
    async fn sequential_questions_each_need_their_own_answer() {
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let tool_a = AskQuestionTool::new(event_tx.clone(), pending.clone());
        let tool_b = AskQuestionTool::new(event_tx, pending.clone());
        let call_a = sample_call("a");
        let call_b = sample_call("b");
        let qid_a = format!("q-{}", call_a.id);
        let qid_b = format!("q-{}", call_b.id);

        // Simulate agent serialization: run A, answer, then B.
        let exec_a = tokio::spawn(async move { tool_a.execute(&call_a).await });
        let _ = event_rx.recv().await;
        {
            let mut m = pending.lock().unwrap();
            m.remove(&qid_a)
                .unwrap()
                .send(QuestionSelection::Suggested)
                .unwrap();
        }
        assert!(exec_a.await.unwrap().success);
        let _ = event_rx.recv().await; // resolved

        let exec_b = tokio::spawn(async move { tool_b.execute(&call_b).await });
        let _ = event_rx.recv().await;
        {
            let mut m = pending.lock().unwrap();
            assert!(m.contains_key(&qid_b));
            assert!(!m.contains_key(&qid_a));
            m.remove(&qid_b)
                .unwrap()
                .send(QuestionSelection::Suggested)
                .unwrap();
        }
        assert!(exec_b.await.unwrap().success);
    }

    #[tokio::test]
    async fn bad_case_rejects_missing_fields() {
        let (event_tx, _event_rx) = mpsc::channel(4);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let tool = AskQuestionTool::new(event_tx, pending);

        let empty_prompt = ToolCall {
            id: "1".into(),
            name: "ask_question".into(),
            input: serde_json::json!({
                "prompt": " ",
                "suggested_answer": "x",
                "options": [{"id":"a","label":"A"}]
            }),
        };
        let r = tool.execute(&empty_prompt).await;
        assert!(!r.success);
        assert!(r.error.as_deref().unwrap().contains("prompt"));

        let empty_suggested = ToolCall {
            id: "2".into(),
            name: "ask_question".into(),
            input: serde_json::json!({
                "prompt": "Q?",
                "suggested_answer": "",
                "options": [{"id":"a","label":"A"}]
            }),
        };
        let r = tool.execute(&empty_suggested).await;
        assert!(!r.success);
        assert!(r.error.as_deref().unwrap().contains("suggested_answer"));

        let no_options = ToolCall {
            id: "3".into(),
            name: "ask_question".into(),
            input: serde_json::json!({
                "prompt": "Q?",
                "suggested_answer": "a",
                "options": []
            }),
        };
        let r = tool.execute(&no_options).await;
        assert!(!r.success);
    }

    #[tokio::test]
    async fn bad_case_unknown_question_id_does_not_resolve_pending() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let tool = AskQuestionTool::new(event_tx, pending.clone());
        let call = sample_call("pending");
        let qid = format!("q-{}", call.id);
        let exec = tokio::spawn(async move { tool.execute(&call).await });
        let _ = event_rx.recv().await;

        // Wrong id — should not unblock
        assert!(pending.lock().unwrap().contains_key(&qid));
        assert!(!pending.lock().unwrap().contains_key("q-nope"));

        {
            let mut m = pending.lock().unwrap();
            m.remove(&qid)
                .unwrap()
                .send(QuestionSelection::Suggested)
                .unwrap();
        }
        assert!(exec.await.unwrap().success);
    }
}
