//! Skill catalog and install/remove/update handlers.

use crate::cmd::util::print_json;
use nca_common::config::NcaConfig;
use nca_core::skills::SkillCatalog;
use std::path::Path;
use std::path::PathBuf;

#[derive(serde::Serialize)]
pub(crate) struct SkillOutput {
    name: String,
    command: String,
    description: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    context: String,
    source: String,
    directory: PathBuf,
}

pub fn list_skills(config: &NcaConfig, workspace_root: &Path, json: bool) -> anyhow::Result<()> {
    let skills = SkillCatalog::discover(workspace_root, &config.harness.skill_directories)
        .map_err(anyhow::Error::msg)?;
    if json {
        let output: Vec<_> = skills
            .into_iter()
            .map(|skill| {
                let source = skill.source_label().to_string();
                SkillOutput {
                    name: skill.name,
                    command: skill.command,
                    description: skill.description,
                    model: skill.model,
                    permission_mode: skill.permission_mode.map(|mode| format!("{mode:?}")),
                    context: format!("{:?}", skill.context),
                    source,
                    directory: skill.directory,
                }
            })
            .collect();
        print_json(&output, false)?;
    } else if skills.is_empty() {
        println!("No skills found");
    } else {
        for skill in skills {
            println!("{}", skill.summary_line());
        }
    }
    Ok(())
}

pub fn handle_skills_add(
    source: &str,
    skill_filter: &[String],
    global: bool,
    workspace_root: &Path,
) -> anyhow::Result<()> {
    use nca_core::skill_installer::{install_skills, parse_source};

    let parsed = parse_source(source).map_err(anyhow::Error::msg)?;
    let installed = install_skills(&parsed, skill_filter, global, workspace_root)
        .map_err(anyhow::Error::msg)?;

    let scope = if global { "(global)" } else { "(local)" };
    println!(
        "Installed {} skill(s) {scope}: {}",
        installed.len(),
        installed.join(", ")
    );
    Ok(())
}

pub fn handle_skills_remove(name: &str, global: bool, workspace_root: &Path) -> anyhow::Result<()> {
    use nca_core::skill_installer::remove_skill;

    remove_skill(name, global, workspace_root).map_err(anyhow::Error::msg)?;
    println!("Removed skill: {name}");
    Ok(())
}

pub fn handle_skills_update(name: Option<&str>, workspace_root: &Path) -> anyhow::Result<()> {
    use nca_core::skill_installer::{
        SkillLock, SkillLockEntry, copy_skill_dir, discover_skills_in_dir, git_clone_to_temp,
        git_head_commit, lock_file_path, skills_dir,
    };

    let mut updated = 0u32;
    let mut up_to_date = 0u32;
    let mut skipped = 0u32;

    for global in [false, true] {
        let lock_path = lock_file_path(global, workspace_root);
        let mut lock = SkillLock::load(&lock_path).map_err(anyhow::Error::msg)?;
        let target = skills_dir(global, workspace_root);
        let mut changed = false;

        let entries: Vec<_> = lock
            .skills
            .iter()
            .filter(|(k, _)| name.is_none() || name == Some(k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (skill_name, entry) in entries {
            if entry.commit.is_none() {
                eprintln!("Skipping '{skill_name}' (installed from local path)");
                skipped += 1;
                continue;
            }

            let clone_url = entry
                .source
                .strip_prefix("github:")
                .map(|repo| format!("https://github.com/{repo}.git"));

            let Some(url) = clone_url else {
                eprintln!("Skipping '{skill_name}' (unknown source format)");
                skipped += 1;
                continue;
            };

            let tmp = git_clone_to_temp(&url).map_err(anyhow::Error::msg)?;
            let new_commit = git_head_commit(tmp.path()).ok();

            if new_commit.as_deref() == entry.commit.as_deref() {
                up_to_date += 1;
                continue;
            }

            let skills_path = if tmp.path().join("skills").is_dir() {
                tmp.path().join("skills")
            } else {
                tmp.path().to_path_buf()
            };

            let discovered = discover_skills_in_dir(&skills_path).map_err(anyhow::Error::msg)?;

            if let Some((_, src_dir)) = discovered.iter().find(|(n, _)| n == &skill_name) {
                let dest = target.join(&skill_name);
                copy_skill_dir(src_dir, &dest).map_err(anyhow::Error::msg)?;
                lock.upsert(
                    &skill_name,
                    SkillLockEntry {
                        source: entry.source.clone(),
                        commit: new_commit,
                        installed_at: chrono::Utc::now().to_rfc3339(),
                    },
                );
                changed = true;
                updated += 1;
            } else {
                eprintln!("Warning: skill '{skill_name}' no longer found in source repo");
                skipped += 1;
            }
        }

        if changed {
            lock.save(&lock_path).map_err(anyhow::Error::msg)?;
        }
    }

    println!("Updated {updated}, already up-to-date {up_to_date}, skipped {skipped}");
    Ok(())
}
