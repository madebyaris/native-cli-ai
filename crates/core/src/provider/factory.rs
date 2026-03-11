use nca_common::config::{NcaConfig, ProviderKind};

use super::minimax::MiniMaxProvider;
use super::{Provider, ProviderError};

/// Build the configured provider for the current workspace.
pub fn build_provider(config: &NcaConfig) -> Result<Box<dyn Provider>, ProviderError> {
    match config.provider.default {
        ProviderKind::MiniMax => Ok(Box::new(MiniMaxProvider::from_config(config)?)),
        ProviderKind::OpenRouter => Err(ProviderError::Configuration(
            "OpenRouter is not implemented yet; switch `provider.default` to `minimax`.".into(),
        )),
        ProviderKind::Anthropic => Err(ProviderError::Configuration(
            "Anthropic is not implemented yet; switch `provider.default` to `minimax`.".into(),
        )),
        ProviderKind::OpenAi => Err(ProviderError::Configuration(
            "OpenAI is not implemented yet; switch `provider.default` to `minimax`.".into(),
        )),
    }
}
