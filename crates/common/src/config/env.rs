//! Environment-variable overrides layered onto [`super::NcaConfig`].

use std::env;

use super::{NcaConfig, ProviderCompatibility, ProviderKind};

impl NcaConfig {
    pub(super) fn apply_env(&mut self) {
        if let Ok(provider) = env::var("NCA_DEFAULT_PROVIDER") {
            self.provider.default = ProviderKind::from_env(&provider);
            self.sync_default_model_from_provider();
        }

        if let Ok(model) = env::var("NCA_MODEL") {
            self.apply_model_override(&model);
        }

        if let Ok(api_key) = env::var("MINIMAX_API_KEY") {
            self.provider.minimax.api_key = Some(api_key);
        }

        if let Ok(base_url) = env::var("MINIMAX_BASE_URL") {
            self.provider.minimax.base_url = base_url;
        }

        if let Ok(model) = env::var("MINIMAX_MODEL") {
            self.provider.minimax.model = model;
        }

        if let Ok(api_key) = env::var("OPENAI_API_KEY") {
            self.provider.openai.api_key = Some(api_key);
        }

        if let Ok(base_url) = env::var("OPENAI_BASE_URL") {
            self.provider.openai.base_url = base_url;
        }

        if let Ok(model) = env::var("OPENAI_MODEL") {
            self.provider.openai.model = model;
        }

        if let Ok(api_key) = env::var("ANTHROPIC_API_KEY") {
            self.provider.anthropic.api_key = Some(api_key);
        }

        if let Ok(base_url) = env::var("ANTHROPIC_BASE_URL") {
            self.provider.anthropic.base_url = base_url;
        }

        if let Ok(model) = env::var("ANTHROPIC_MODEL") {
            self.provider.anthropic.model = model;
        }

        if let Ok(api_key) = env::var("OPENROUTER_API_KEY") {
            self.provider.openrouter.api_key = Some(api_key);
        }

        if let Ok(base_url) = env::var("OPENROUTER_BASE_URL") {
            self.provider.openrouter.base_url = base_url;
        }

        if let Ok(model) = env::var("OPENROUTER_MODEL") {
            self.provider.openrouter.model = model;
        }

        if let Ok(site_url) = env::var("OPENROUTER_SITE_URL") {
            self.provider.openrouter.site_url = Some(site_url);
        }

        if let Ok(app_name) = env::var("OPENROUTER_APP_NAME") {
            self.provider.openrouter.app_name = Some(app_name);
        }

        if let Ok(api_key) = env::var("CUSTOM_PROVIDER_API_KEY") {
            self.provider.custom.api_key = Some(api_key);
        }

        if let Ok(base_url) = env::var("CUSTOM_PROVIDER_BASE_URL") {
            self.provider.custom.base_url = base_url;
        }

        if let Ok(model) = env::var("CUSTOM_PROVIDER_MODEL") {
            self.provider.custom.model = model;
        }

        if let Ok(raw) = env::var("CUSTOM_PROVIDER_COMPATIBILITY")
            && let Some(compatibility) = ProviderCompatibility::from_cli_name(&raw)
        {
            self.provider.custom.compatibility = compatibility;
        }

        if let Ok(memory_path) = env::var("NCA_MEMORY_PATH") {
            self.memory.file_path = memory_path.into();
        }

        if let Ok(timeout_secs) = env::var("NCA_WEB_TIMEOUT_SECS")
            && let Ok(timeout_secs) = timeout_secs.parse()
        {
            self.web.timeout_secs = timeout_secs;
        }

        if let Ok(max_fetch_chars) = env::var("NCA_WEB_MAX_FETCH_CHARS")
            && let Ok(max_fetch_chars) = max_fetch_chars.parse()
        {
            self.web.max_fetch_chars = max_fetch_chars;
        }

        self.sync_default_model_from_provider();
    }
}

#[cfg(test)]
mod tests {
    use super::super::{NcaConfig, ProviderCompatibility, ProviderKind};
    use std::env;

    struct EnvGuard {
        previous: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn set(vars: &[(&str, Option<&str>)]) -> Self {
            let mut previous = Vec::new();
            for (key, value) in vars {
                previous.push((key.to_string(), env::var(key).ok()));
                match value {
                    Some(value) => unsafe { env::set_var(key, value) },
                    None => unsafe { env::remove_var(key) },
                }
            }
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.previous.drain(..) {
                match value {
                    Some(value) => unsafe { env::set_var(&key, value) },
                    None => unsafe { env::remove_var(&key) },
                }
            }
        }
    }

    #[test]
    fn apply_env_supports_openai_anthropic_openrouter_and_custom() {
        let _guard = EnvGuard::set(&[
            ("NCA_DEFAULT_PROVIDER", Some("openrouter")),
            ("OPENAI_API_KEY", Some("openai-key")),
            ("OPENAI_MODEL", Some("gpt-4o")),
            ("ANTHROPIC_API_KEY", Some("anthropic-key")),
            ("ANTHROPIC_MODEL", Some("claude-3-7-sonnet-20250219")),
            ("OPENROUTER_API_KEY", Some("openrouter-key")),
            ("OPENROUTER_MODEL", Some("anthropic/claude-3.7-sonnet")),
            ("OPENROUTER_SITE_URL", Some("https://nca.test")),
            ("OPENROUTER_APP_NAME", Some("Native CLI AI")),
            ("CUSTOM_PROVIDER_API_KEY", Some("custom-key")),
            ("CUSTOM_PROVIDER_BASE_URL", Some("https://custom.example")),
            ("CUSTOM_PROVIDER_MODEL", Some("custom-model-x")),
            ("CUSTOM_PROVIDER_COMPATIBILITY", Some("anthropic")),
        ]);

        let mut config = NcaConfig::default();
        config.apply_env();

        assert_eq!(config.provider.default, ProviderKind::OpenRouter);
        assert_eq!(
            config.provider.openai.resolve_api_key().as_deref(),
            Some("openai-key")
        );
        assert_eq!(
            config.provider.anthropic.resolve_api_key().as_deref(),
            Some("anthropic-key")
        );
        assert_eq!(
            config.provider.openrouter.resolve_api_key().as_deref(),
            Some("openrouter-key")
        );
        assert_eq!(config.provider.openai.model, "gpt-4o");
        assert_eq!(
            config.provider.anthropic.model,
            "claude-3-7-sonnet-20250219"
        );
        assert_eq!(
            config.provider.openrouter.model,
            "anthropic/claude-3.7-sonnet"
        );
        assert_eq!(
            config.provider.openrouter.site_url.as_deref(),
            Some("https://nca.test")
        );
        assert_eq!(
            config.provider.openrouter.app_name.as_deref(),
            Some("Native CLI AI")
        );
        assert_eq!(
            config.provider.custom.resolve_api_key().as_deref(),
            Some("custom-key")
        );
        assert_eq!(config.provider.custom.base_url, "https://custom.example");
        assert_eq!(config.provider.custom.model, "custom-model-x");
        assert_eq!(
            config.provider.custom.compatibility,
            ProviderCompatibility::Anthropic
        );
        assert_eq!(config.model.default_model, "anthropic/claude-3.7-sonnet");
    }
}
