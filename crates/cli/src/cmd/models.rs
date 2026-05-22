//! Model catalog display.

use crate::cmd::util::print_json;
use nca_common::config::{ModelRetryConfig, NcaConfig, ProviderKind};
use std::path::PathBuf;

#[derive(serde::Serialize)]
pub(crate) struct ModelCatalogOutput {
    default_provider: String,
    default_model: String,
    provider_models: Vec<ProviderModelOutput>,
    aliases: std::collections::BTreeMap<String, String>,
    thinking_enabled: bool,
    thinking_budget: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry: Option<ModelRetryConfig>,
}

#[derive(serde::Serialize)]
pub(crate) struct ProviderModelOutput {
    provider: String,
    model: String,
    base_url: String,
    selected: bool,
}

pub fn show_models(config: &NcaConfig, json: bool, verbose: bool) -> anyhow::Result<()> {
    let output = ModelCatalogOutput {
        default_provider: config.provider.default.display_name().to_string(),
        default_model: config.model.default_model.clone(),
        provider_models: ProviderKind::ALL
            .into_iter()
            .map(|provider| ProviderModelOutput {
                provider: provider.display_name().to_string(),
                model: config.provider.model_for(provider).to_string(),
                base_url: config.provider.base_url_for(provider).to_string(),
                selected: provider == config.provider.default,
            })
            .collect(),
        aliases: config.model.aliases.clone(),
        thinking_enabled: config.model.enable_thinking,
        thinking_budget: config.model.thinking_budget,
        retry: verbose.then(|| config.model.retry.clone()),
    };
    if json {
        print_json(&output, false)?;
    } else {
        println!(
            "Default provider/model: {} / {}",
            output.default_provider, output.default_model
        );
        println!(
            "Thinking: {} (budget {})",
            if output.thinking_enabled { "on" } else { "off" },
            output.thinking_budget
        );
        println!("Provider models:");
        for provider in &output.provider_models {
            println!(
                "  {}{} -> {} ({})",
                provider.provider,
                if provider.selected { " [selected]" } else { "" },
                provider.model,
                provider.base_url
            );
        }
        for (alias, target) in output.aliases {
            println!("  {alias} -> {target}");
        }
        if verbose {
            let r = &config.model.retry;
            println!(
                "Empty-response retries: max {} (exponential backoff {}ms → cap {}ms)",
                r.max_empty_response_retries,
                r.empty_response_backoff_initial_ms,
                r.empty_response_backoff_max_ms
            );
        }
    }
    Ok(())
}
