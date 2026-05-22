//! Workspace init and config display.

use crate::cmd::memory::workspace_memory_store;
use crate::cmd::util::print_json;
use nca_common::config::{NcaConfig, ProviderKind};
use std::path::Path;

pub fn run_init(workspace_root: &Path, force: bool) -> anyhow::Result<()> {
    let nca_dir = workspace_root.join(".nca");
    let ws_config = nca_common::config::workspace_config_path(workspace_root);

    if !nca_dir.exists() {
        std::fs::create_dir_all(&nca_dir)?;
        println!("  ✓ created {}", nca_dir.display());
    } else {
        println!("  · {} already exists", nca_dir.display());
    }

    let sessions_dir = nca_dir.join("sessions");
    if !sessions_dir.exists() {
        std::fs::create_dir_all(&sessions_dir)?;
        println!("  ✓ created {}", sessions_dir.display());
    }

    let skills_dir = nca_dir.join("skills");
    if !skills_dir.exists() {
        std::fs::create_dir_all(&skills_dir)?;
        println!("  ✓ created {}", skills_dir.display());
    }

    if ws_config.exists() && !force {
        println!(
            "  · {} already exists (use --force to overwrite)",
            ws_config.display()
        );
    } else {
        let default = nca_common::config::NcaConfig::default();
        let toml = toml::to_string_pretty(&default)
            .map_err(|err| anyhow::anyhow!("serialize default config: {err}"))?;
        std::fs::write(&ws_config, toml)?;
        println!(
            "  ✓ wrote default workspace config to {}",
            ws_config.display()
        );
    }

    let gitignore = nca_dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(
            &gitignore,
            "sessions/\nworktrees/\n.last_session\nmemory.json\n",
        )?;
        println!("  ✓ wrote {}", gitignore.display());
    }

    println!("\nWorkspace initialized. Run `nca doctor` to verify, or `nca` to start.");
    Ok(())
}

pub fn show_config(config: &NcaConfig, workspace_root: &Path, json: bool) -> anyhow::Result<()> {
    if json {
        print_json(config, false)?;
    } else {
        println!(
            "Global config: {}",
            nca_common::config::global_config_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unavailable>".into())
        );
        println!(
            "Workspace config: {}",
            nca_common::config::workspace_config_path(workspace_root).display()
        );
        println!("Default provider: {:?}", config.provider.default);
        println!("Default model: {}", config.model.default_model);
        println!("Permission mode: {:?}", config.permissions.mode);
        println!("Provider endpoints:");
        for provider in ProviderKind::ALL {
            println!(
                "  {} -> model={} base_url={}",
                provider.display_name(),
                config.provider.model_for(provider),
                config.provider.base_url_for(provider)
            );
        }
        println!(
            "Memory path: {}",
            workspace_memory_store(config, workspace_root)
                .path()
                .display()
        );
        println!("Skill directories:");
        for path in &config.harness.skill_directories {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                workspace_root.join(path)
            };
            println!("  {}", resolved.display());
        }
    }
    Ok(())
}
