use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("invalid plugin manifest: {0}")]
    Manifest(String),
    #[error("plugin install failed: {0}")]
    Install(String),
    #[error("plugin `{plugin}` failed: {message}")]
    Runtime { plugin: String, message: String },
    #[error("plugin registry error: {0}")]
    Registry(String),
}
