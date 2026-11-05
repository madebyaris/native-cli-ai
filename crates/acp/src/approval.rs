//! ACP `session/request_permission` bridge for nca's approval policy.

use crate::translate;
use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    SessionId, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Client, ConnectionTo};
use nca_common::tool::ToolCall;
use nca_core::approval::suggest_allow_pattern;
use nca_core::approval::{ApprovalHandler, ApprovalVerdict};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct AcpApprovalHandler {
    session_id: SessionId,
    cx: ConnectionTo<Client>,
    workspace: PathBuf,
    cancel: Arc<AtomicBool>,
    /// Patterns the user rejected with "Reject for this session".
    session_deny: Mutex<Vec<String>>,
}

impl AcpApprovalHandler {
    pub fn new(
        session_id: SessionId,
        cx: ConnectionTo<Client>,
        workspace: PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        Self {
            session_id,
            cx,
            workspace,
            cancel,
            session_deny: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for AcpApprovalHandler {
    async fn resolve(&self, call: &ToolCall, description: &str) -> ApprovalVerdict {
        if self.cancel.load(Ordering::SeqCst) {
            return ApprovalVerdict::Denied;
        }
        let pattern = suggest_allow_pattern(&call.name, &call.input);
        if self.denied_for_session(&pattern) {
            return ApprovalVerdict::Denied;
        }

        let title = if description.trim().is_empty() {
            translate::tool_title(&call.name, &call.input)
        } else {
            description.to_string()
        };
        let tool_call = ToolCallUpdate::new(
            call.id.clone(),
            ToolCallUpdateFields::new()
                .title(Some(title))
                .name(Some(call.name.clone()))
                .kind(translate::tool_kind(&call.name))
                .status(agent_client_protocol::schema::v1::ToolCallStatus::Pending)
                .locations(translate::tool_locations(
                    &call.name,
                    &call.input,
                    &self.workspace,
                ))
                .raw_input(Some(call.input.clone())),
        );

        let options = vec![
            PermissionOption::new("allow_once", "Allow once", PermissionOptionKind::AllowOnce),
            PermissionOption::new(
                "allow_always",
                "Allow for this session",
                PermissionOptionKind::AllowAlways,
            ),
            PermissionOption::new("reject_once", "Reject", PermissionOptionKind::RejectOnce),
            PermissionOption::new(
                "reject_always",
                "Reject for this session",
                PermissionOptionKind::RejectAlways,
            ),
        ];

        let request = RequestPermissionRequest::new(self.session_id.clone(), tool_call, options);
        let response = match self.cx.send_request(request).block_task().await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!("permission request failed: {error}");
                return ApprovalVerdict::Denied;
            }
        };

        match response.outcome {
            RequestPermissionOutcome::Cancelled => ApprovalVerdict::Denied,
            RequestPermissionOutcome::Selected(selected) => match selected.option_id.0.as_ref() {
                "allow_once" => ApprovalVerdict::Approved,
                "allow_always" => ApprovalVerdict::AllowPattern(pattern),
                "reject_always" => {
                    if let Ok(mut deny) = self.session_deny.lock() {
                        deny.push(pattern);
                    }
                    ApprovalVerdict::Denied
                }
                _ => ApprovalVerdict::Denied,
            },
            _ => ApprovalVerdict::Denied,
        }
    }
}

impl AcpApprovalHandler {
    fn denied_for_session(&self, pattern: &str) -> bool {
        self.session_deny
            .lock()
            .is_ok_and(|deny| deny.iter().any(|denied| denied == pattern))
    }
}
