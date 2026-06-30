use nca_common::config::{NcaConfig, ProviderKind};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use super::anthropic::AnthropicProvider;
use super::minimax::MiniMaxProvider;
use super::openai_compat::{CompatProfile, OpenAiCompatProvider};
use super::{Provider, ProviderError};

const OPENAI_PROFILE: CompatProfile = CompatProfile {
    name: "OpenAI",
    endpoint_suffix: "v1/chat/completions",
    strip_reasoning: false,
};

const OPENROUTER_PROFILE: CompatProfile = CompatProfile {
    name: "OpenRouter",
    endpoint_suffix: "v1/chat/completions",
    strip_reasoning: false,
};

const ZHIPUAI_PROFILE: CompatProfile = CompatProfile {
    name: "ZhipuAI",
    endpoint_suffix: "chat/completions",
    strip_reasoning: false,
};

const DEEPSEEK_PROFILE: CompatProfile = CompatProfile {
    name: "DeepSeek",
    endpoint_suffix: "chat/completions",
    strip_reasoning: true,
};

/// Build the configured provider for the current workspace (uses `config.provider.default`).
pub fn build_provider(config: &NcaConfig) -> Result<Box<dyn Provider>, ProviderError> {
    build_provider_for(config, config.provider.default)
}

/// Build a provider for a specific [`ProviderKind`], ignoring `config.provider.default`.
///
/// This is used when an agent profile or skill specifies a different provider than
/// the session default.
pub fn build_provider_for(
    config: &NcaConfig,
    kind: ProviderKind,
) -> Result<Box<dyn Provider>, ProviderError> {
    match kind {
        ProviderKind::MiniMax => Ok(Box::new(MiniMaxProvider::from_config(config)?)),
        ProviderKind::OpenRouter => {
            let mut extra = HeaderMap::new();
            if let Some(url) = &config.provider.openrouter.site_url {
                let _ = extra.insert(
                    HeaderName::from_static("http-referer"),
                    HeaderValue::from_str(url).unwrap(),
                );
            }
            if let Some(name) = &config.provider.openrouter.app_name {
                let _ = extra.insert(
                    HeaderName::from_static("x-title"),
                    HeaderValue::from_str(name).unwrap(),
                );
            }
            Ok(Box::new(OpenAiCompatProvider::from_config(
                &config.provider.openrouter,
                config.model.max_tokens,
                OPENROUTER_PROFILE,
                extra,
            )?))
        }
        ProviderKind::Anthropic => Ok(Box::new(AnthropicProvider::from_config(config)?)),
        ProviderKind::OpenAi => {
            let extra = HeaderMap::new();
            Ok(Box::new(OpenAiCompatProvider::from_config(
                &config.provider.openai,
                config.model.max_tokens,
                OPENAI_PROFILE,
                extra,
            )?))
        }
        ProviderKind::ZhipuAI => {
            let extra = HeaderMap::new();
            Ok(Box::new(OpenAiCompatProvider::from_config(
                &config.provider.zhipuai,
                config.model.max_tokens,
                ZHIPUAI_PROFILE,
                extra,
            )?))
        }
        ProviderKind::DeepSeek => {
            let extra = HeaderMap::new();
            Ok(Box::new(OpenAiCompatProvider::from_config(
                &config.provider.deepseek,
                config.model.max_tokens,
                DEEPSEEK_PROFILE,
                extra,
            )?))
        }
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
                ProviderKind::ZhipuAI => {
                    config.provider.zhipuai.api_key = Some("zhipuai-key".into());
                }
                ProviderKind::DeepSeek => {
                    config.provider.deepseek.api_key = Some("deepseek-key".into());
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
                assert!(
                    matches!(error, ProviderError::Configuration(message) if message.contains("missing OpenAI API key"))
                );
            }
        }
    }

    #[test]
    fn build_provider_for_uses_explicit_kind_not_default() {
        let mut config = NcaConfig::default();
        // Default is minimax, but we request openai explicitly
        config.provider.default = ProviderKind::MiniMax;
        config.provider.minimax.api_key = Some("minimax-key".into());
        config.provider.openai.api_key = Some("openai-key".into());

        // build_provider follows default (minimax)
        assert!(build_provider(&config).is_ok());

        // build_provider_for can override to openai
        let provider = build_provider_for(&config, ProviderKind::OpenAi);
        assert!(provider.is_ok(), "expected openai provider to build");
    }
}
