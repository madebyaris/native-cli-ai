use nca_common::config::{NcaConfig, ProviderKind};

use super::anthropic::AnthropicProvider;
use super::minimax::MiniMaxProvider;
use super::openai::OpenAiProvider;
use super::openrouter::OpenRouterProvider;
use super::{Provider, ProviderError};

/// Build the configured provider for the current workspace.
pub fn build_provider(config: &NcaConfig) -> Result<Box<dyn Provider>, ProviderError> {
    match config.provider.default {
        ProviderKind::MiniMax => Ok(Box::new(MiniMaxProvider::from_config(config)?)),
        ProviderKind::OpenRouter => Ok(Box::new(OpenRouterProvider::from_config(config)?)),
        ProviderKind::Anthropic => Ok(Box::new(AnthropicProvider::from_config(config)?)),
        ProviderKind::OpenAi => Ok(Box::new(OpenAiProvider::from_config(config)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_builds_each_supported_provider_when_configured() {
        for kind in ProviderKind::ALL {
            let mut config = NcaConfig::default();
            config.provider.default = kind;
            match kind {
                ProviderKind::MiniMax => {
                    config.provider.minimax.api_key = Some("minimax-key".into());
                }
                ProviderKind::OpenAi => {
                    config.provider.openai.api_key = Some("openai-key".into());
                }
                ProviderKind::Anthropic => {
                    config.provider.anthropic.api_key = Some("anthropic-key".into());
                }
                ProviderKind::OpenRouter => {
                    config.provider.openrouter.api_key = Some("openrouter-key".into());
                }
            }

            let provider = build_provider(&config);
            assert!(
                provider.is_ok(),
                "expected provider {:?} to build, got {:?}",
                kind,
                provider.as_ref().err()
            );
        }
    }

    #[test]
    fn factory_fails_loudly_when_selected_provider_is_missing_credentials() {
        let mut config = NcaConfig::default();
        config.provider.default = ProviderKind::OpenAi;
        match build_provider(&config) {
            Ok(_) => panic!("missing credentials should fail"),
            Err(error) => {
                assert!(matches!(error, ProviderError::Configuration(message) if message.contains("missing OpenAI API key")));
            }
        }
    }
}
