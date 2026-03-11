//! Reduces AgentEvent stream into SessionVm for UI rendering.

use nca_common::event::{AgentEvent, EndReason, EventEnvelope};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::state::{TimelineEntry, ToolCallState};

/// Reduces a single event into updates for SessionVm.
/// Returns (timeline_entry, pending_approval, tool_update, cost_update, error, session_ended, resolve_approval_call_id).
#[allow(clippy::type_complexity)]
pub fn reduce_event(
    event: &AgentEvent,
) -> (
    Option<TimelineEntry>,
    Option<(String, String, String)>,
    Option<(String, ToolCallState)>,
    Option<(u64, u64, f64)>,
    Option<String>,
    Option<EndReason>,
    Option<String>,
) {
    match event {
        AgentEvent::SessionStarted {
            session_id,
            model,
            workspace,
        } => (
            Some(TimelineEntry::SessionStarted {
                session_id: session_id.clone(),
                model: model.clone(),
                workspace: workspace.display().to_string(),
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        AgentEvent::MessageReceived { role, content } => (
            Some(TimelineEntry::Message {
                role: role.clone(),
                content: content.clone(),
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        AgentEvent::TokensStreamed { delta } => (
            Some(TimelineEntry::Tokens {
                delta: delta.clone(),
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        AgentEvent::ToolCallStarted {
            call_id,
            tool,
            input,
        } => (
            Some(TimelineEntry::ToolStart {
                call_id: call_id.clone(),
                tool: tool.clone(),
                input: input.clone(),
            }),
            None,
            Some((
                call_id.clone(),
                ToolCallState::Started {
                    tool: tool.clone(),
                    input: input.clone(),
                    output: None,
                },
            )),
            None,
            None,
            None,
            None,
        ),
        AgentEvent::ToolCallCompleted { call_id, output } => (
            Some(TimelineEntry::ToolComplete {
                call_id: call_id.clone(),
                output: output.clone(),
            }),
            None,
            Some((
                call_id.clone(),
                ToolCallState::Completed {
                    output: output.clone(),
                },
            )),
            None,
            None,
            None,
            None,
        ),
        AgentEvent::ApprovalRequested {
            call_id,
            tool,
            description,
        } => (
            Some(TimelineEntry::ApprovalRequested {
                call_id: call_id.clone(),
                tool: tool.clone(),
                description: description.clone(),
            }),
            Some((call_id.clone(), tool.clone(), description.clone())),
            None,
            None,
            None,
            None,
            None,
        ),
        AgentEvent::ApprovalResolved { call_id, approved } => (
            Some(TimelineEntry::ApprovalResolved {
                call_id: call_id.clone(),
                approved: *approved,
            }),
            None,
            None,
            None,
            None,
            None,
            Some(call_id.clone()),
        ),
        AgentEvent::CostUpdated {
            input_tokens,
            output_tokens,
            estimated_cost_usd,
        } => (
            Some(TimelineEntry::Cost {
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
                estimated_cost_usd: *estimated_cost_usd,
            }),
            None,
            None,
            Some((*input_tokens, *output_tokens, *estimated_cost_usd)),
            None,
            None,
            None,
        ),
        AgentEvent::Checkpoint {
            phase,
            detail,
            turn,
        } => (
            Some(TimelineEntry::Checkpoint {
                phase: phase.clone(),
                detail: detail.clone(),
                turn: *turn,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        AgentEvent::SessionEnded { reason } => (
            Some(TimelineEntry::SessionEnded {
                reason: reason.clone(),
            }),
            None,
            None,
            None,
            None,
            Some(reason.clone()),
            None,
        ),
        AgentEvent::Error { message } => (
            Some(TimelineEntry::Error {
                message: message.clone(),
            }),
            None,
            None,
            None,
            Some(message.clone()),
            None,
            None,
        ),
        AgentEvent::Response { response } => (
            Some(TimelineEntry::Message {
                role: "system".into(),
                content: serde_json::to_string(response).unwrap_or_default(),
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        AgentEvent::ChildSessionSpawned {
            child_session_id,
            task,
            ..
        } => (
            Some(TimelineEntry::ChildSpawned {
                child_session_id: child_session_id.clone(),
                task: task.clone(),
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        AgentEvent::ChildSessionCompleted {
            child_session_id,
            status,
            ..
        } => (
            Some(TimelineEntry::ChildCompleted {
                child_session_id: child_session_id.clone(),
                status: status.clone(),
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
    }
}

/// Replay events from a .events.jsonl file into a SessionVm.
pub fn replay_events_file(path: &Path) -> crate::state::SessionVm {
    let mut vm = crate::state::SessionVm::new();
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return vm,
    };
    let reader = BufReader::new(file);
    for line in reader.lines().flatten() {
        let event = serde_json::from_str::<EventEnvelope>(&line)
            .map(|e| e.event)
            .or_else(|_| serde_json::from_str::<AgentEvent>(&line));
        if let Ok(event) = event {
            let (te, pa, tu, cu, err, se, resolve) = reduce_event(&event);
            if let Some(call_id) = resolve {
                vm.resolve_approval(&call_id);
            }
            vm.apply(te, pa, tu, cu, err, se);
        }
    }
    vm
}
