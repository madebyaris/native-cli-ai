//! JSON-RPC-like protocol for out-of-process plugin communication.
//!
//! Transport: NDJSON over Unix socket.
//! Each message is one line of JSON terminated by '\n'.
//!
//! Future: replace JSON with Cap'n Proto — same logical schema, different wire format.

use serde::{Deserialize, Serialize};

/// Request sent from nca host to plugin process.
#[derive(Debug, Serialize, Deserialize)]
pub struct PluginRequest {
    pub id: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// Response sent from plugin process back to nca host.
#[derive(Debug, Serialize, Deserialize)]
pub struct PluginResponse {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parameters for the `system_prompt` method.
#[derive(Debug, Serialize, Deserialize)]
pub struct SystemPromptParams {
    pub workspace_root: String,
    // Limited config snapshot: just what a plugin needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
}

/// Result from the `system_prompt` method.
/// `text` = Some(string) to inject, None to skip.
#[derive(Debug, Serialize, Deserialize)]
pub struct SystemPromptResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Hello message sent by plugin on connect.
#[derive(Debug, Serialize, Deserialize)]
pub struct HelloMessage {
    pub name: String,
    pub version: String,
}

impl PluginRequest {
    pub fn new_system_prompt(id: impl Into<String>, workspace_root: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            method: "system_prompt".into(),
            params: Some(serde_json::json!(SystemPromptParams {
                workspace_root: workspace_root.into(),
                permission_mode: None,
            })),
        }
    }

    pub fn new_shutdown(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            method: "shutdown".into(),
            params: None,
        }
    }
}

impl PluginResponse {
    pub fn success(id: impl Into<String>, result: Option<serde_json::Value>) -> Self {
        Self {
            id: id.into(),
            result,
            error: None,
        }
    }

    pub fn error(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            result: None,
            error: Some(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_and_deserializes() {
        let req = PluginRequest::new_system_prompt("1", "/workspace");
        let json = serde_json::to_string(&req).unwrap();
        let parsed: PluginRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "1");
        assert_eq!(parsed.method, "system_prompt");
        assert!(parsed.params.is_some());
    }

    #[test]
    fn response_serializes_and_deserializes() {
        let resp = PluginResponse::success("1", Some(serde_json::json!({"text": "plugin output"})));
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: PluginResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "1");
        assert!(parsed.result.is_some());
        assert!(parsed.error.is_none());
    }

    #[test]
    fn response_null_result() {
        let resp = PluginResponse::success("1", None);
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: PluginResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "1");
        assert!(parsed.result.is_none());
    }

    #[test]
    fn response_error() {
        let resp = PluginResponse::error("1", "something went wrong");
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: PluginResponse = serde_json::from_str(&json).unwrap();
        assert!(parsed.result.is_none());
        assert_eq!(parsed.error.as_deref(), Some("something went wrong"));
    }
}
