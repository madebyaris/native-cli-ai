use nca_common::config::PermissionMode;
use serde::Deserialize;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: Option<String>,
    pub command: String,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub context: SkillContextMode,
    pub directory: PathBuf,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillContextMode {
    Inline,
    Fork,
}

pub struct SkillCatalog;

impl SkillCatalog {
    pub fn discover(
        workspace_root: &Path,
        skill_directories: &[PathBuf],
    ) -> Result<Vec<Skill>, String> {
        let mut roots = Vec::new();
        if let Some(home) = env::var_os("HOME") {
            let home = PathBuf::from(home);
            roots.push(home.join(".nca/skills"));
            roots.push(home.join(".claude/skills"));
        }

        for dir in skill_directories {
            if dir.is_absolute() {
                roots.push(dir.clone());
            } else {
                roots.push(workspace_root.join(dir));
            }
        }

        let mut skills = Vec::new();
        for root in roots {
            if !root.exists() {
                continue;
            }
            let entries = std::fs::read_dir(&root)
                .map_err(|err| format!("failed to read skills dir {}: {err}", root.display()))?;
            for entry in entries {
                let entry = entry.map_err(|err| err.to_string())?;
                let path = entry.path();
                let skill_file = if path.is_dir() {
                    path.join("SKILL.md")
                } else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                    path.clone()
                } else {
                    continue;
                };
                if !skill_file.exists() {
                    continue;
                }
                if let Ok(skill) = parse_skill_file(&skill_file) {
                    if !skills.iter().any(|existing: &Skill| existing.command == skill.command) {
                        skills.push(skill);
                    }
                }
            }
        }

        skills.sort_by(|left, right| left.command.cmp(&right.command));
        Ok(skills)
    }
}

impl Skill {
    pub fn summary_line(&self) -> String {
        match &self.description {
            Some(description) => format!("/{:<14} {}", self.command, description),
            None => format!("/{:<14} {}", self.command, self.name),
        }
    }

    pub fn prompt_for_task(&self, task: &str) -> String {
        let mut prompt = format!(
            "Use the skill `{}`.\n\nSkill instructions:\n{}\n",
            self.command,
            self.body.trim()
        );
        if !task.trim().is_empty() {
            prompt.push_str(&format!("\nTask:\n{}\n", task.trim()));
        }
        prompt
    }

    pub fn manifest_summary(&self) -> String {
        let description = self
            .description
            .as_deref()
            .unwrap_or("No description provided.");
        let model = self.model.as_deref().unwrap_or("inherit");
        let permission_mode = self
            .permission_mode
            .map(|mode| format!("{mode:?}"))
            .unwrap_or_else(|| "inherit".into());
        format!(
            "- /{}: {}\n  model={model} permission_mode={permission_mode} context={:?}",
            self.command, description, self.context
        )
    }
}

fn parse_skill_file(path: &Path) -> Result<Skill, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let directory = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_stem = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill")
        .to_string();

    let (frontmatter, body) = split_frontmatter(&raw)?;
    let command = frontmatter
        .command
        .clone()
        .unwrap_or_else(|| slugify(&frontmatter.name.clone().unwrap_or_else(|| file_stem.clone())));
    Ok(Skill {
        name: frontmatter.name.unwrap_or(file_stem),
        description: frontmatter.description,
        command,
        model: frontmatter.model,
        permission_mode: frontmatter.permission_mode,
        context: frontmatter.context.unwrap_or(SkillContextMode::Inline),
        directory,
        body: body.trim().to_string(),
    })
}

fn split_frontmatter(raw: &str) -> Result<(SkillFrontmatter, String), String> {
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let yaml = &rest[..end];
            let body = &rest[end + 5..];
            let fm = serde_yaml::from_str::<SkillFrontmatter>(yaml)
                .map_err(|err| format!("failed to parse skill frontmatter: {err}"))?;
            return Ok((fm, body.to_string()));
        }
    }
    Ok((SkillFrontmatter::default(), raw.to_string()))
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    command: Option<String>,
    model: Option<String>,
    permission_mode: Option<PermissionMode>,
    context: Option<SkillContextMode>,
}

impl<'de> Deserialize<'de> for SkillContextMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "fork" => Ok(Self::Fork),
            _ => Ok(Self::Inline),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_frontmatter_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Review PR\ndescription: Review code changes\ncommand: review\nmodel: MiniMax-M2.5\npermission_mode: plan\ncontext: fork\n---\nInspect diffs first.\n",
        )
        .unwrap();

        let skill = parse_skill_file(&skill_dir.join("SKILL.md")).unwrap();
        assert_eq!(skill.command, "review");
        assert_eq!(skill.context, SkillContextMode::Fork);
        assert_eq!(skill.permission_mode, Some(PermissionMode::Plan));
        assert!(skill.body.contains("Inspect diffs"));
    }
}
