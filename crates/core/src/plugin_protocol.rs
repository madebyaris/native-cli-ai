//! Cap'n Proto wire protocol for out-of-process plugin communication.
//!
//! Transport: self-delimiting Cap'n Proto streaming frames over stdin/stdout.
//! stderr is inherited for plugin diagnostics.
//!
//! Schema: `schema/plugin.capnp` — see `plugin_capnp` module for generated types.

use capnp::serialize;
use std::io;

use crate::plugin_capnp::{body, plugin_message};

/// Current plugin protocol version.
pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 0;

/// Error type for plugin wire operations.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("capnp error: {0}")]
    Capnp(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl From<capnp::Error> for WireError {
    fn from(e: capnp::Error) -> Self {
        WireError::Capnp(e.to_string())
    }
}

impl From<capnp::NotInSchema> for WireError {
    fn from(e: capnp::NotInSchema) -> Self {
        WireError::Capnp(e.to_string())
    }
}

impl From<core::str::Utf8Error> for WireError {
    fn from(e: core::str::Utf8Error) -> Self {
        WireError::Capnp(e.to_string())
    }
}

/// Read a single Cap'n Proto `PluginMessage` and process it via a callback.
///
/// This avoids lifetime issues — the callback receives the typed reader
/// while the message segments are still alive.
pub fn read_message_then<R, F, T>(reader: &mut R, f: F) -> Result<T, WireError>
where
    R: io::BufRead,
    F: for<'a> FnOnce(plugin_message::Reader<'a>) -> Result<T, WireError>,
{
    let message_reader = serialize::read_message(reader, capnp::message::ReaderOptions::new())?;
    let root = message_reader.get_root::<plugin_message::Reader<'_>>()?;
    f(root)
}

/// Write a `PluginMessage` builder to a stream (stdout/pipe).
pub fn write_message<W: io::Write>(
    writer: &mut W,
    message: &capnp::message::Builder<capnp::message::HeapAllocator>,
) -> Result<(), WireError> {
    serialize::write_message(writer, message)?;
    Ok(())
}

/// Build a `PluginMessage` wire frame as a `Vec<u8>`.
///
/// The closure receives a `body::Builder` to set the appropriate union variant.
pub fn build_message<F>(id: &str, set_body: F) -> Vec<u8>
where
    F: FnOnce(&mut body::Builder<'_>),
{
    let mut message = capnp::message::Builder::new_default();
    {
        let mut root: plugin_message::Builder<'_> = message.init_root();
        root.set_id(id);
        // init_body() consumes root's body and returns a body::Builder.
        // We pass &mut to the closure so it can set the union variant.
        let mut b = root.reborrow().init_body();
        set_body(&mut b);
    }
    let mut buf = Vec::new();
    serialize::write_message(&mut buf, &message).expect("serialize message");
    buf
}

/// Extract a human-readable method name from a `body::Reader` for logging.
pub fn body_method_name(body: &body::Reader<'_>) -> &'static str {
    match body.which() {
        Ok(body::Hello(_)) => "hello",
        Ok(body::Config(_)) => "config",
        Ok(body::Shutdown(_)) => "shutdown",
        Ok(body::RefreshCapabilities(_)) => "refreshCapabilities",
        Ok(body::ExecuteTool(_)) => "executeTool",
        Ok(body::SystemPrompt(_)) => "systemPrompt",
        Ok(body::UserPrompt(_)) => "userPrompt",
        Ok(body::ChatParams(_)) => "chatParams",
        Ok(body::ChatMessagesTransform(_)) => "chatMessagesTransform",
        Ok(body::ShellEnv(_)) => "shellEnv",
        Ok(body::ToolDefinition(_)) => "toolDefinition",
        Ok(body::PermissionAsk(_)) => "permissionAsk",
        Ok(body::ToolExecuteBefore(_)) => "toolExecuteBefore",
        Ok(body::ToolExecuteAfter(_)) => "toolExecuteAfter",
        Ok(body::CommandExecuteBefore(_)) => "commandExecuteBefore",
        Ok(body::Event(_)) => "event",
        Ok(body::ExecuteToolResult(_)) => "executeToolResult",
        Ok(body::SystemPromptResult(_)) => "systemPromptResult",
        Ok(body::UserPromptResult(_)) => "userPromptResult",
        Ok(body::ChatParamsResult(_)) => "chatParamsResult",
        Ok(body::ChatMessagesTransformResult(_)) => "chatMessagesTransformResult",
        Ok(body::ShellEnvResult(_)) => "shellEnvResult",
        Ok(body::ToolDefinitionResult(_)) => "toolDefinitionResult",
        Ok(body::PermissionAskResult(_)) => "permissionAskResult",
        Ok(body::ToolExecuteBeforeResult(_)) => "toolExecuteBeforeResult",
        Ok(body::ToolExecuteAfterResult(_)) => "toolExecuteAfterResult",
        Ok(body::CommandExecuteBeforeResult(_)) => "commandExecuteBeforeResult",
        Ok(body::CapabilitiesResult(_)) => "capabilitiesResult",
        Ok(body::ReadFile(_)) => "readFile",
        Ok(body::ListDirectory(_)) => "listDirectory",
        Ok(body::SearchCode(_)) => "searchCode",
        Ok(body::GetWorkspaceRoot(_)) => "getWorkspaceRoot",
        Ok(body::Log(_)) => "log",
        Ok(body::ReadFileResponse(_)) => "readFileResponse",
        Ok(body::ListDirectoryResponse(_)) => "listDirectoryResponse",
        Ok(body::SearchCodeResponse(_)) => "searchCodeResponse",
        Ok(body::GetWorkspaceRootResponse(_)) => "getWorkspaceRootResponse",
        Ok(body::LogResponse(_)) => "logResponse",
        Ok(body::Error(_)) => "error",
        Err(_) => "unknown",
    }
}

// ── Convenience builders for common messages ─────────────────────────────────

/// Build a shutdown message.
pub fn build_shutdown(id: &str) -> Vec<u8> {
    build_message(id, |body| {
        body.reborrow().set_shutdown(());
    })
}

/// Build a config message.
pub fn build_config(
    id: &str,
    workspace_root: &str,
    session_id: &str,
    permission_mode: &str,
    global_skills_dir: &str,
) -> Vec<u8> {
    build_message(id, |body| {
        let mut cfg = body.reborrow().init_config();
        cfg.set_workspace_root(workspace_root);
        cfg.set_session_id(session_id);
        cfg.set_permission_mode(permission_mode);
        cfg.set_global_skills_dir(global_skills_dir);
    })
}

/// Build an error response.
pub fn build_error(id: &str, message: &str) -> Vec<u8> {
    build_message(id, |body| {
        body.reborrow().init_error().set_message(message);
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_message_round_trip() {
        let wire = build_shutdown("42");
        let mut reader = io::BufReader::new(&wire[..]);
        let result = read_message_then(&mut reader, |msg| {
            assert_eq!(msg.get_id()?, "42");
            let body = msg.get_body()?;
            assert!(matches!(body.which(), Ok(body::Shutdown(_))));
            Ok(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn error_message_round_trip() {
        let wire = build_error("5", "something broke");
        let mut reader = io::BufReader::new(&wire[..]);
        let result = read_message_then(&mut reader, |msg| {
            let body = msg.get_body()?;
            match body.which() {
                Ok(body::Error(e)) => {
                    let e = e?;
                    assert_eq!(e.get_message()?, "something broke");
                }
                _ => panic!("expected Error"),
            }
            Ok(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn config_message_round_trip() {
        let wire = build_config(
            "1",
            "/workspace",
            "session-1",
            "default",
            "/home/user/.config/nca/skills",
        );
        let mut reader = io::BufReader::new(&wire[..]);
        let result = read_message_then(&mut reader, |msg| {
            assert_eq!(msg.get_id()?, "1");
            let body = msg.get_body()?;
            match body.which() {
                Ok(body::Config(c)) => {
                    let c = c?;
                    assert_eq!(c.get_workspace_root()?, "/workspace");
                    assert_eq!(c.get_session_id()?, "session-1");
                    assert_eq!(c.get_permission_mode()?, "default");
                    assert_eq!(c.get_global_skills_dir()?, "/home/user/.config/nca/skills");
                }
                _ => panic!("expected Config"),
            }
            Ok(())
        });
        assert!(result.is_ok());
    }
}
