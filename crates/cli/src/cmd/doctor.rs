//! Doctor diagnostics and auto-fix.

use crate::cmd::util::print_json;
use nca_common::config::{NcaConfig, ProviderKind};
use nca_core::skills::SkillCatalog;
use std::path::{Path, PathBuf};

#[derive(serde::Serialize)]
pub(crate) struct DoctorOutput {
    provider: String,
    default_model: String,
    providers: Vec<ProviderDoctorStatus>,
    mcp_server_count: usize,
    skill_count: usize,
    memory_path: PathBuf,
}

#[derive(serde::Serialize)]
pub(crate) struct ProviderDoctorStatus {
    provider: String,
    selected: bool,
    api_key_present: bool,
    api_key_env: String,
    model: String,
    base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    preflight_ms: Option<u64>,
    /// `ok`, `invalid_key`, `network_error`, `skipped`, `skipped_no_api_key`, …
    preflight: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    preflight_detail: Option<String>,
}

pub async fn show_doctor(
    config: &NcaConfig,
    workspace_root: &Path,
    json: bool,
    fix: bool,
) -> anyhow::Result<()> {
    let skip_net = std::env::var("NCA_DOCTOR_SKIP_NETWORK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let skills = SkillCatalog::discover(workspace_root, &config.harness.skill_directories)
        .map(|skills| skills.len())
        .unwrap_or(0);

    let mut providers = Vec::new();
    for provider in ProviderKind::ALL {
        let base = ProviderDoctorStatus {
            provider: provider.display_name().to_string(),
            selected: provider == config.provider.default,
            api_key_present: config.provider.api_key_present_for(provider),
            api_key_env: config.provider.api_key_env_for(provider).to_string(),
            model: config.provider.model_for(provider).to_string(),
            base_url: config.provider.base_url_for(provider).to_string(),
            preflight_ms: None,
            preflight: "skipped_no_api_key".to_string(),
            preflight_detail: None,
        };

        let row = if skip_net {
            ProviderDoctorStatus {
                preflight_ms: None,
                preflight: "skipped".to_string(),
                preflight_detail: Some("NCA_DOCTOR_SKIP_NETWORK set — no HTTP probe".to_string()),
                ..base
            }
        } else if !config.provider.api_key_present_for(provider) {
            base
        } else {
            let key = config
                .provider
                .resolve_api_key_for(provider)
                .unwrap_or_default();
            let url = config.provider.base_url_for(provider).to_string();
            let compat = config.provider.compatibility_for(provider);
            let report =
                nca_core::provider::validate::preflight_provider(provider, &key, &url, compat)
                    .await;
            let (preflight, detail) = match &report.validation {
                nca_core::provider::validate::ValidationResult::Valid => ("ok".to_string(), None),
                nca_core::provider::validate::ValidationResult::InvalidKey(m) => {
                    ("invalid_key".to_string(), Some(m.clone()))
                }
                nca_core::provider::validate::ValidationResult::NetworkError(m) => {
                    ("network_error".to_string(), Some(m.clone()))
                }
            };
            ProviderDoctorStatus {
                preflight_ms: Some(report.round_trip_ms),
                preflight,
                preflight_detail: detail,
                ..base
            }
        };
        providers.push(row);
    }

    let output = DoctorOutput {
        provider: config.provider.default.display_name().to_string(),
        default_model: config.model.default_model.clone(),
        providers,
        mcp_server_count: config
            .mcp
            .servers
            .iter()
            .filter(|server| server.enabled)
            .count(),
        skill_count: skills,
        memory_path: if config.memory.file_path.is_absolute() {
            config.memory.file_path.clone()
        } else {
            workspace_root.join(&config.memory.file_path)
        },
    };
    if json {
        print_json(&output, false)?;
    } else {
        println!("Provider: {}", output.provider);
        println!("Default model: {}", output.default_model);
        println!("Provider readiness (preflight uses a lightweight API probe when a key is set):");
        for provider in &output.providers {
            let pre = if let Some(ms) = provider.preflight_ms {
                format!(
                    " preflight={}ms ({})",
                    ms,
                    provider.preflight.replace('_', " ")
                )
            } else {
                format!(" preflight=— ({})", provider.preflight.replace('_', " "))
            };
            println!(
                "  {}{}: api_key={} ({}) model={} base_url={}{}",
                provider.provider,
                if provider.selected { " [selected]" } else { "" },
                if provider.api_key_present {
                    "configured"
                } else {
                    "missing"
                },
                provider.api_key_env,
                provider.model,
                provider.base_url,
                pre
            );
            if let Some(d) = &provider.preflight_detail {
                println!("      note: {d}");
            }
        }
        println!("Skills discovered: {}", output.skill_count);
        println!("MCP servers enabled: {}", output.mcp_server_count);
        println!("Memory path: {}", output.memory_path.display());
        println!("MiniMax remains the default recommended path for this workspace.");
    }

    if fix {
        let applied = run_doctor_fixes(config, workspace_root)?;
        if !applied.is_empty() {
            println!("\nApplied fixes:");
            for msg in applied {
                println!("  • {msg}");
            }
        } else {
            println!("\nNo fixes needed — workspace already set up.");
        }
    }
    Ok(())
}

pub fn run_doctor_fixes(config: &NcaConfig, workspace_root: &Path) -> anyhow::Result<Vec<String>> {
    let mut actions = Vec::new();

    let nca_dir = workspace_root.join(".nca");
    if !nca_dir.exists() {
        std::fs::create_dir_all(&nca_dir)?;
        actions.push(format!("created {}", nca_dir.display()));
    }

    let history = if config.session.history_dir.is_absolute() {
        config.session.history_dir.clone()
    } else {
        workspace_root.join(&config.session.history_dir)
    };
    if !history.exists() {
        std::fs::create_dir_all(&history)?;
        actions.push(format!("created session history dir {}", history.display()));
    }

    let memory_path = if config.memory.file_path.is_absolute() {
        config.memory.file_path.clone()
    } else {
        workspace_root.join(&config.memory.file_path)
    };
    if !memory_path.exists() {
        if let Some(parent) = memory_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&memory_path, "[]\n")?;
        actions.push(format!("seeded memory file {}", memory_path.display()));
    }

    for dir in &config.harness.skill_directories {
        let resolved = if dir.is_absolute() {
            dir.clone()
        } else {
            workspace_root.join(dir)
        };
        if !resolved.exists() {
            std::fs::create_dir_all(&resolved)?;
            actions.push(format!("created skill dir {}", resolved.display()));
        }
    }

    Ok(actions)
}
