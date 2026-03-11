use std::path::Path;

/// Handle to a multiplexer session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub name: String,
}

/// Abstraction over terminal multiplexers (tmux, zellij, etc.).
#[async_trait::async_trait]
pub trait MultiplexerAdapter: Send + Sync {
    async fn create_session(&self, name: &str, cwd: &Path) -> Result<SessionHandle, MultiplexerError>;
    async fn attach(&self, handle: &SessionHandle) -> Result<(), MultiplexerError>;
    async fn detach(&self, handle: &SessionHandle) -> Result<(), MultiplexerError>;
    async fn send_keys(&self, handle: &SessionHandle, keys: &str) -> Result<(), MultiplexerError>;
    async fn capture_pane(&self, handle: &SessionHandle) -> Result<String, MultiplexerError>;
    async fn kill_session(&self, handle: &SessionHandle) -> Result<(), MultiplexerError>;
}

/// Phase 3: tmux adapter using tmux_interface.
pub struct TmuxAdapter;

#[async_trait::async_trait]
impl MultiplexerAdapter for TmuxAdapter {
    async fn create_session(&self, _name: &str, _cwd: &Path) -> Result<SessionHandle, MultiplexerError> {
        Err(MultiplexerError::NotImplemented)
    }
    async fn attach(&self, _handle: &SessionHandle) -> Result<(), MultiplexerError> {
        Err(MultiplexerError::NotImplemented)
    }
    async fn detach(&self, _handle: &SessionHandle) -> Result<(), MultiplexerError> {
        Err(MultiplexerError::NotImplemented)
    }
    async fn send_keys(&self, _handle: &SessionHandle, _keys: &str) -> Result<(), MultiplexerError> {
        Err(MultiplexerError::NotImplemented)
    }
    async fn capture_pane(&self, _handle: &SessionHandle) -> Result<String, MultiplexerError> {
        Err(MultiplexerError::NotImplemented)
    }
    async fn kill_session(&self, _handle: &SessionHandle) -> Result<(), MultiplexerError> {
        Err(MultiplexerError::NotImplemented)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MultiplexerError {
    #[error("Multiplexer adapter not yet implemented")]
    NotImplemented,
    #[error("Tmux error: {0}")]
    TmuxError(String),
}
