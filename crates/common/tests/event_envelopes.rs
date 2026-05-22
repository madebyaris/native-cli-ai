//! Snapshot tests for NDJSON event envelope serialization.
//!
//! Guarantees that the on-disk event log format and IPC wire format stay
//! stable across refactors. Uses fixed ids and a pinned timestamp so the
//! output is deterministic.

use chrono::{DateTime, Utc};
use nca_common::event::{
    AgentCommand, AgentEvent, AgentResponse, BusyState, EndReason, EventEnvelope,
    InteractiveQuestionPayload, QuestionOption, QuestionSelection, ToolOutputStream,
};
use nca_common::session::{SessionMeta, SessionState, SessionStatus};
use nca_common::todo::{TodoItem, TodoStatus};
use nca_common::tool::ToolResult;
use serde_json::json;
use std::path::PathBuf;

fn pinned_ts() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn envelope(id: u64, event: AgentEvent) -> EventEnvelope {
    EventEnvelope {
        schema_version: 1,
        id,
        ts: Some(pinned_ts()),
        event,
    }
}

fn ndjson(envelopes: &[EventEnvelope]) -> String {
    envelopes
        .iter()
        .map(|e| serde_json::to_string(e).expect("serialize"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn sample_session_meta() -> SessionMeta {
    SessionMeta {
        id: "sess-sample".into(),
        created_at: pinned_ts(),
        updated_at: pinned_ts(),
        workspace: PathBuf::from("/tmp/workspace"),
        model: "MiniMax-M2.5".into(),
        status: SessionStatus::Running,
        pid: Some(4242),
        socket_path: Some(PathBuf::from("/tmp/nca/sess-sample.sock")),
        worktree_path: None,
        branch: None,
        base_branch: None,
        parent_session_id: None,
        child_session_ids: vec![],
        inherited_summary: None,
        spawn_reason: None,
        session_summary: None,
        orchestration: None,
    }
}

fn sample_session_state() -> SessionState {
    SessionState {
        schema_version: 1,
        meta: sample_session_meta(),
        messages: vec![],
        total_input_tokens: 10,
        total_output_tokens: 5,
        estimated_cost_usd: 0.0001,
    }
}

#[allow(clippy::too_many_lines)]
fn all_agent_events() -> Vec<AgentEvent> {
    let question = InteractiveQuestionPayload {
        question_id: "q-1".into(),
        call_id: "call-q".into(),
        prompt: "Pick one".into(),
        options: vec![
            QuestionOption {
                id: "a".into(),
                label: "Alpha".into(),
            },
            QuestionOption {
                id: "b".into(),
                label: "Beta".into(),
            },
        ],
        allow_custom: true,
        suggested_answer: "Alpha".into(),
    };

    vec![
        AgentEvent::SessionStarted {
            session_id: "sess-abc".into(),
            workspace: PathBuf::from("/tmp/workspace"),
            model: "MiniMax-M2.5".into(),
        },
        AgentEvent::MessageReceived {
            role: "user".into(),
            content: "hello".into(),
        },
        AgentEvent::TokensStreamed {
            delta: "Hello".into(),
        },
        AgentEvent::ToolCallStarted {
            call_id: "call-1".into(),
            tool: "list_files".into(),
            input: json!({ "glob": "src/**/*.rs" }),
        },
        AgentEvent::ToolOutputChunk {
            call_id: "call-1".into(),
            stream: ToolOutputStream::Stdout,
            data: "line1\n".into(),
        },
        AgentEvent::ToolCallCompleted {
            call_id: "call-1".into(),
            output: ToolResult {
                call_id: "call-1".into(),
                success: true,
                output: "src/main.rs".into(),
                error: None,
            },
        },
        AgentEvent::ApprovalRequested {
            call_id: "call-2".into(),
            tool: "write_file".into(),
            description: "Write README".into(),
        },
        AgentEvent::ApprovalResolved {
            call_id: "call-2".into(),
            approved: true,
        },
        AgentEvent::CostUpdated {
            input_tokens: 128,
            output_tokens: 64,
            estimated_cost_usd: 0.000_192,
        },
        AgentEvent::Checkpoint {
            phase: "turn-complete".into(),
            detail: "persisted session state".into(),
            turn: 3,
        },
        AgentEvent::SessionEnded {
            reason: EndReason::Completed,
        },
        AgentEvent::Error {
            message: "something failed".into(),
        },
        AgentEvent::Response {
            response: AgentResponse::Ok,
        },
        AgentEvent::ChildSessionSpawned {
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
            task: "refactor parser".into(),
            workspace: PathBuf::from("/tmp/ws"),
            branch: Some("nca/child-1".into()),
        },
        AgentEvent::ChildSessionCompleted {
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
            status: "completed".into(),
        },
        AgentEvent::ChildSessionActivity {
            child_session_id: "child-1".into(),
            phase: "tool".into(),
            detail: "read_file src/lib.rs".into(),
        },
        AgentEvent::QuestionRequested {
            question: question.clone(),
        },
        AgentEvent::QuestionResolved {
            question_id: "q-1".into(),
            selection: QuestionSelection::Option {
                option_id: "a".into(),
            },
        },
        AgentEvent::ContextWarning {
            message: "approaching limit".into(),
        },
        AgentEvent::ContextCompaction {
            phase: "summarize".into(),
            message: "compacted history".into(),
        },
        AgentEvent::BusyStateChanged {
            state: BusyState::Streaming,
        },
        AgentEvent::TodosUpdated {
            session_id: "sess-abc".into(),
            todos: vec![TodoItem {
                id: "t-1".into(),
                content: "Add tests".into(),
                status: TodoStatus::InProgress,
                created_at: pinned_ts(),
                updated_at: pinned_ts(),
            }],
        },
    ]
}

#[test]
fn all_agent_event_variants_roundtrip_json() {
    for (idx, event) in all_agent_events().into_iter().enumerate() {
        let env = envelope(idx as u64 + 1, event);
        let json = serde_json::to_string(&env).expect("serialize");
        let back: EventEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.schema_version, 1);
        assert_eq!(back.id, idx as u64 + 1);
        let json2 = serde_json::to_string(&back.event).expect("re-serialize event");
        let original_json = serde_json::to_string(&env.event).expect("serialize event");
        assert_eq!(json2, original_json);
    }
}

#[test]
fn agent_command_variants_roundtrip_and_snapshot() {
    let commands = vec![
        AgentCommand::SendMessage {
            content: "hello".into(),
        },
        AgentCommand::ApproveToolCall {
            call_id: "call-1".into(),
        },
        AgentCommand::DenyToolCall {
            call_id: "call-2".into(),
        },
        AgentCommand::AnswerQuestion {
            question_id: "q-1".into(),
            selection: QuestionSelection::Suggested,
        },
        AgentCommand::Cancel,
        AgentCommand::Shutdown,
    ];

    for cmd in &commands {
        let json = serde_json::to_string(cmd).expect("serialize command");
        let back: AgentCommand = serde_json::from_str(&json).expect("deserialize command");
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    insta::assert_json_snapshot!("agent_commands_all_variants", commands);
}

#[test]
fn agent_response_variants_roundtrip_and_snapshot() {
    let responses = vec![
        AgentResponse::SessionState {
            session: Box::new(sample_session_state()),
        },
        AgentResponse::SessionList {
            sessions: vec![sample_session_meta()],
        },
        AgentResponse::Error {
            message: "not found".into(),
        },
        AgentResponse::Ok,
    ];

    for resp in &responses {
        let json = serde_json::to_string(resp).expect("serialize response");
        let back: AgentResponse = serde_json::from_str(&json).expect("deserialize response");
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    insta::assert_json_snapshot!("agent_responses_all_variants", responses);
}

#[test]
fn ndjson_event_log_stable_shape() {
    let events = vec![
        envelope(
            1,
            AgentEvent::SessionStarted {
                session_id: "sess-abc".into(),
                workspace: "/tmp/workspace".into(),
                model: "MiniMax-M2.5".into(),
            },
        ),
        envelope(
            2,
            AgentEvent::BusyStateChanged {
                state: BusyState::Streaming,
            },
        ),
        envelope(
            3,
            AgentEvent::TokensStreamed {
                delta: "Hello".into(),
            },
        ),
        envelope(
            4,
            AgentEvent::ToolCallStarted {
                call_id: "call-1".into(),
                tool: "list_files".into(),
                input: json!({ "glob": "src/**/*.rs" }),
            },
        ),
        envelope(
            5,
            AgentEvent::ToolCallCompleted {
                call_id: "call-1".into(),
                output: ToolResult {
                    call_id: "call-1".into(),
                    success: true,
                    output: "src/main.rs\nsrc/lib.rs".into(),
                    error: None,
                },
            },
        ),
        envelope(
            6,
            AgentEvent::CostUpdated {
                input_tokens: 128,
                output_tokens: 64,
                estimated_cost_usd: 0.000_192,
            },
        ),
        envelope(
            7,
            AgentEvent::SessionEnded {
                reason: EndReason::Completed,
            },
        ),
    ];

    let log = ndjson(&events);
    insta::assert_snapshot!("ndjson_event_log", log);
}

#[test]
fn ndjson_event_envelope_json_shape() {
    let env = envelope(
        42,
        AgentEvent::Checkpoint {
            phase: "turn-complete".into(),
            detail: "persisted session state".into(),
            turn: 3,
        },
    );

    insta::assert_json_snapshot!("ndjson_single_envelope", env);
}
