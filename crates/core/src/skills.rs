use nca_common::config::{PermissionMode, ProviderKind};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: Option<String>,
    pub command: String,
    pub model: Option<String>,
    /// Explicit provider override (e.g. `provider: openai`).
    /// When set, skill invocations route to this provider instead of the session default.
    pub provider: Option<ProviderKind>,
    pub permission_mode: Option<PermissionMode>,
    pub context: SkillContextMode,
    /// If `true`, this skill auto-registers as a named agent profile so it can be
    /// used as a specialist via `spawn_subagent(specialist=...)`.
    pub agent: bool,
    pub directory: PathBuf,
    pub body: String,
    pub source: SkillSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillContextMode {
    Inline,
    Fork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    AgentsMd,
    FileSystem,
}

pub struct SkillCatalog;

impl SkillCatalog {
    pub fn discover(
        workspace_root: &Path,
        skill_directories: &[PathBuf],
    ) -> Result<Vec<Skill>, String> {
        let mut roots = Vec::new();
        if let Some(config_dir) = nca_common::config::xdg_config_dir() {
            roots.push(config_dir.join("nca/skills"));
            roots.push(config_dir.join("claude/skills"));
        }

        for dir in skill_directories {
            if dir.is_absolute() {
                roots.push(dir.clone());
            } else {
                roots.push(workspace_root.join(dir));
            }
        }

        let mut skills = Vec::new();

        // Parse AGENTS.md first (takes precedence on conflicts)
        if let Ok(agents_skills) = parse_agents_md(workspace_root) {
            skills.extend(agents_skills);
        }

        // Then add filesystem skills (skip if command already exists)
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
                if let Ok(skill) = parse_skill_file(&skill_file)
                    && !skills
                        .iter()
                        .any(|existing: &Skill| existing.command == skill.command)
                {
                    skills.push(skill);
                }
            }
        }

        skills.sort_by(|left, right| left.command.cmp(&right.command));
        Ok(skills)
    }
}

impl Skill {
    pub fn summary_line(&self) -> String {
        let source_tag = match self.source {
            SkillSource::AgentsMd => " [AGENTS.md]",
            SkillSource::FileSystem => "",
        };
        match &self.description {
            Some(description) => format!("/{:<14} {}{}", self.command, description, source_tag),
            None => format!("/{:<14} {}{}", self.command, self.name, source_tag),
        }
    }

    pub fn prompt_for_task(&self, task: &str) -> String {
        let mut prompt = format!(
            "Use the skill `{}`.\n\nSkill instructions:\n{}\n",
            self.command,
            self.expanded_body().trim()
        );
        if !task.trim().is_empty() {
            prompt.push_str(&format!("\nTask:\n{}\n", task.trim()));
        }
        prompt
    }

    pub fn manifest_summary(&self) -> String {
        let raw_description = self
            .description
            .as_deref()
            .unwrap_or("No description provided.");
        // Clip description to keep the skill index compact in the system prompt.
        const MAX_DESC_CHARS: usize = 120;
        let description = if raw_description.len() > MAX_DESC_CHARS {
            let clipped: String = raw_description.chars().take(MAX_DESC_CHARS).collect();
            format!("{clipped}…")
        } else {
            raw_description.to_string()
        };
        let model = self.model.as_deref().unwrap_or("inherit");
        let provider = self
            .provider
            .as_ref()
            .map(|p| format!("{p:?}").to_ascii_lowercase())
            .unwrap_or_else(|| "inherit".into());
        let permission_mode = self
            .permission_mode
            .map(|mode| format!("{mode:?}"))
            .unwrap_or_else(|| "inherit".into());
        let source_tag = match self.source {
            SkillSource::AgentsMd => " [AGENTS.md]",
            SkillSource::FileSystem => "",
        };
        format!(
            "- /{}: {}{}\n  model={model} provider={provider} permission_mode={permission_mode} context={:?}",
            self.command, description, source_tag, self.context
        )
    }

    pub fn source_label(&self) -> &'static str {
        match self.source {
            SkillSource::AgentsMd => "agents-md",
            SkillSource::FileSystem => "filesystem",
        }
    }

    /// Return the skill body with referenced supporting files inlined.
    ///
    /// Scans the body for file references (./path, @path, backtick-wrapped paths),
    /// resolves them against the skill's directory, and appends their contents.
    /// Bare filenames (no `/` or `./` prefix) resolve only against skill directory (Level 1).
    pub fn expanded_body(&self) -> String {
        let refs = extract_file_references(&self.body);
        if refs.is_empty() {
            return self.body.clone();
        }

        let mut expanded = self.body.clone();
        for ref_path in &refs {
            let clean = ref_path.trim_start_matches("./");

            // Block directory traversal
            if clean.contains("..") {
                continue;
            }

            let is_bare_filename = !clean.contains('/');

            let resolved = if is_bare_filename {
                // Pattern 4: bare filenames resolve only against skill directory (Level 1)
                let candidate = self.directory.join(clean);
                if candidate.is_file() {
                    Some(candidate)
                } else {
                    None
                }
            } else {
                resolve_skill_reference(ref_path, &self.directory)
            };

            if let Some(resolved) = resolved
                && let Ok(content) = std::fs::read_to_string(&resolved)
            {
                expanded.push_str(&format!("\n\n===== {} =====\n\n{}", clean, content.trim()));
            }
        }

        expanded
    }
}

fn parse_skill_file(path: &Path) -> Result<Skill, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
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
    let command = frontmatter.command.clone().unwrap_or_else(|| {
        slugify(
            &frontmatter
                .name
                .clone()
                .unwrap_or_else(|| file_stem.clone()),
        )
    });
    Ok(Skill {
        name: frontmatter.name.unwrap_or(file_stem),
        description: frontmatter.description,
        command,
        model: frontmatter.model,
        provider: frontmatter.provider,
        permission_mode: frontmatter.permission_mode,
        context: frontmatter.context.unwrap_or(SkillContextMode::Inline),
        agent: frontmatter.agent.unwrap_or(false),
        directory,
        body: body.trim().to_string(),
        source: SkillSource::FileSystem,
    })
}

fn split_frontmatter(raw: &str) -> Result<(SkillFrontmatter, String), String> {
    if let Some(rest) = raw.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---\n")
    {
        let yaml = &rest[..end];
        let body = &rest[end + 5..];
        let fm = serde_yaml::from_str::<SkillFrontmatter>(yaml)
            .map_err(|err| format!("failed to parse skill frontmatter: {err}"))?;
        return Ok((fm, body.to_string()));
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
    provider: Option<ProviderKind>,
    permission_mode: Option<PermissionMode>,
    context: Option<SkillContextMode>,
    /// If `true`, auto-register this skill as a named agent profile.
    #[serde(default)]
    agent: Option<bool>,
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

/// Parse AGENTS.md as a skill manifest.
/// Each `## <Heading>` becomes a skill, with optional frontmatter.
fn parse_agents_md(workspace_root: &Path) -> Result<Vec<Skill>, String> {
    let agents_path = workspace_root.join("AGENTS.md");
    if !agents_path.exists() {
        return Ok(Vec::new());
    }

    let raw = std::fs::read_to_string(&agents_path)
        .map_err(|err| format!("failed to read AGENTS.md: {err}"))?;

    let mut skills = Vec::new();
    let mut current_heading = String::new();
    let mut current_content = String::new();
    let mut in_frontmatter = false;
    let mut frontmatter_lines = Vec::new();

    for line in raw.lines() {
        if line.starts_with("## ") {
            // Save previous skill if exists
            if !current_heading.is_empty() {
                if let Some(skill) = build_skill_from_section(
                    &current_heading,
                    &frontmatter_lines.join("\n"),
                    current_content.trim(),
                    workspace_root,
                ) {
                    skills.push(skill);
                }
                frontmatter_lines.clear();
                in_frontmatter = false;
            }
            current_heading = line.trim_start_matches("## ").trim().to_string();
            current_content.clear();
        } else if line.trim().is_empty() {
            // Empty line - if in frontmatter, stay in frontmatter; otherwise accumulate
            if !in_frontmatter && (!current_content.is_empty() || !current_heading.is_empty()) {
                current_content.push('\n');
            }
        } else if line.starts_with("- ") && !in_frontmatter && frontmatter_lines.is_empty() {
            // First directive line - check if it's a skill frontmatter directive
            let trimmed = line.trim_start_matches("- ");
            if trimmed.starts_with("model=")
                || trimmed.starts_with("provider=")
                || trimmed.starts_with("permission_mode=")
                || trimmed.starts_with("context=")
                || trimmed.starts_with("agent=")
            {
                in_frontmatter = true;
                frontmatter_lines.push(trimmed);
            } else {
                // Not a frontmatter directive, accumulate as content
                current_content.push_str(line);
                current_content.push('\n');
            }
        } else if in_frontmatter {
            if line.starts_with("- ") {
                frontmatter_lines.push(line.trim_start_matches("- "));
            } else {
                // Non-directive line ends frontmatter
                in_frontmatter = false;
                current_content.push_str(line);
                current_content.push('\n');
            }
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    // Don't forget the last section
    if !current_heading.is_empty()
        && let Some(skill) = build_skill_from_section(
            &current_heading,
            &frontmatter_lines.join("\n"),
            current_content.trim(),
            workspace_root,
        )
    {
        skills.push(skill);
    }

    Ok(skills)
}

fn build_skill_from_section(
    heading: &str,
    frontmatter: &str,
    body: &str,
    workspace_root: &Path,
) -> Option<Skill> {
    let command = slugify(heading);
    if command.is_empty() {
        return None;
    }

    let mut model = None;
    let mut provider = None;
    let mut permission_mode = None;
    let mut context = SkillContextMode::Inline;
    let mut agent = false;

    // Parse frontmatter lines: model=inherit permission_mode=inherit context=Inline
    // Each line may contain multiple directives separated by spaces
    for line in frontmatter.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Split by spaces to handle "model=inherit permission_mode=plan"
        for directive in line.split_whitespace() {
            if directive.starts_with("provider=") {
                let val = directive.trim_start_matches("provider=").trim();
                provider = ProviderKind::from_cli_name(val);
            } else if directive.starts_with("model=") {
                let val = directive.trim_start_matches("model=").trim();
                if val != "inherit" {
                    model = Some(val.to_string());
                }
            } else if directive.starts_with("permission_mode=") {
                let val = directive.trim_start_matches("permission_mode=").trim();
                permission_mode = val.parse::<PermissionMode>().ok();
            } else if directive.starts_with("context=") {
                let val = directive
                    .trim_start_matches("context=")
                    .trim()
                    .to_lowercase();
                if val == "fork" {
                    context = SkillContextMode::Fork;
                }
            } else if directive.starts_with("agent=") {
                let val = directive.trim_start_matches("agent=").trim().to_lowercase();
                agent = val == "true";
            }
        }
    }

    Some(Skill {
        name: heading.to_string(),
        description: Some(section_description(heading, body)),
        command,
        model,
        provider,
        permission_mode,
        context,
        agent,
        directory: workspace_root.to_path_buf(),
        body: body.to_string(),
        source: SkillSource::AgentsMd,
    })
}

fn section_description(heading: &str, body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            line.strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .unwrap_or(line)
                .trim()
                .to_string()
        })
        .unwrap_or_else(|| heading.to_string())
}

/// Resolve a file reference to an absolute path using three-level strategy:
/// 1. Relative to skill directory
/// 2. Relative to catalog root (parent of skill directory)
/// 3. Strip `skills/` prefix and retry against catalog root
///
/// Returns `None` if the file doesn't exist at any level.
/// Rejects paths containing `..` to prevent directory traversal.
fn resolve_skill_reference(reference: &str, skill_directory: &Path) -> Option<PathBuf> {
    let clean = reference.trim_start_matches("./");

    // Block directory traversal
    if clean.contains("..") {
        return None;
    }

    // Level 1: relative to skill directory
    let level1 = skill_directory.join(clean);
    if level1.is_file() {
        return Some(level1);
    }

    // Level 2: relative to catalog root (parent of skill directory)
    // Only for filesystem skills (not AGENTS.md skills whose directory is workspace root).
    // We detect this by checking if skill_directory has a parent that is different
    // from the directory itself (AGENTS.md skills use workspace_root directly).
    if let Some(catalog_root) = skill_directory.parent() {
        // Skip Level 2/3 if skill_directory looks like a workspace root
        // (i.e., it doesn't have a "skills"-like parent structure).
        // Filesystem skills are always nested: <catalog_root>/<skill_name>/SKILL.md
        // AGENTS.md skills have directory == workspace_root, so parent is workspace parent.
        let skill_dir_name = skill_directory
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let is_likely_skill_subdir =
            !skill_dir_name.is_empty() && skill_dir_name != "." && skill_directory != catalog_root;

        if is_likely_skill_subdir {
            let level2 = catalog_root.join(clean);
            if level2.is_file() {
                return Some(level2);
            }

            // Level 3: strip "skills/" prefix and retry against catalog root
            if let Some(stripped) = clean.strip_prefix("skills/")
                && !stripped.contains("..")
            {
                let level3 = catalog_root.join(stripped);
                if level3.is_file() {
                    return Some(level3);
                }
            }
        }
    }

    None
}

/// Extract file references from a skill body.
///
/// Detects: `./path`, `@path`, backtick-wrapped paths with supported extensions.
/// Returns deduplicated list in first-occurrence order, with `@` stripped and `./` preserved.
fn extract_file_references(body: &str) -> Vec<String> {
    use regex::Regex;
    use std::collections::HashSet;
    use std::sync::LazyLock;

    static EXCLUDED_NAMES: &[&str] = &[
        "CLAUDE.md",
        "AGENTS.md",
        "GEMINI.md",
        "SKILL.md",
        "README.md",
        "package.json",
    ];

    static DOT_SLASH_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\./[a-zA-Z0-9_][a-zA-Z0-9_./-]*\.(md|sh|ts|js|dot|txt|html|cjs)").unwrap()
    });

    // Require @ at start of line or after whitespace to avoid matching emails
    static AT_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:^|\s)@([a-zA-Z0-9_][a-zA-Z0-9_./-]*\.(md|sh|ts|js|dot|txt|html|cjs))")
            .unwrap()
    });

    static BACKTICK_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"`([a-zA-Z0-9_][a-zA-Z0-9_./-]*\.(md|sh|ts|js|dot|txt|html|cjs))`").unwrap()
    });

    fn is_excluded(path: &str) -> bool {
        EXCLUDED_NAMES.contains(&path)
            || path.contains("path/to/")
            || path.starts_with("http://")
            || path.starts_with("https://")
    }

    let mut seen = HashSet::new();
    let mut refs = Vec::new();

    // Pattern 1: ./path references
    for m in DOT_SLASH_RE.find_iter(body) {
        let path = m.as_str().to_string();
        let key = path.trim_start_matches("./").to_string();
        if !is_excluded(&key) && !seen.contains(&key) {
            seen.insert(key);
            refs.push(path);
        }
    }

    // Pattern 2: @path references (strip @, require word boundary)
    for cap in AT_RE.captures_iter(body) {
        let path = cap[1].to_string();
        let key = path.clone();
        if !is_excluded(&key) && !seen.contains(&key) {
            seen.insert(key);
            refs.push(path);
        }
    }

    // Pattern 3+4: backtick-wrapped paths
    for cap in BACKTICK_RE.captures_iter(body) {
        let path = cap[1].to_string();
        let key = path.trim_start_matches("./").to_string();
        if !is_excluded(&key) && !EXCLUDED_NAMES.contains(&path.as_str()) && !seen.contains(&key) {
            seen.insert(key);
            refs.push(path);
        }
    }

    refs
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
            "---\nname: Review PR\ndescription: Review code changes\ncommand: review\nmodel: MiniMax-M2.7\npermission_mode: plan\ncontext: fork\n---\nInspect diffs first.\n",
        )
        .unwrap();

        let skill = parse_skill_file(&skill_dir.join("SKILL.md")).unwrap();
        assert_eq!(skill.command, "review");
        assert_eq!(skill.context, SkillContextMode::Fork);
        assert_eq!(skill.permission_mode, Some(PermissionMode::Plan));
        assert!(skill.body.contains("Inspect diffs"));
    }

    #[test]
    fn parses_agents_md_as_skills() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            r#"## Learned User Preferences

- Always use Rust-native solutions

Some detailed content here.

## Drizzle ORM

- model=inherit permission_mode=inherit context=Inline

Type-safe SQL ORM for TypeScript with zero runtime overhead

## Shadcn UI

- model=inherit permission_mode=plan context=fork

Component management and styling...
"#,
        )
        .unwrap();

        let skills = parse_agents_md(dir.path()).unwrap();
        assert_eq!(skills.len(), 3);

        // Check "Learned User Preferences" became a skill
        let pref = skills
            .iter()
            .find(|s| s.command == "learned-user-preferences")
            .unwrap();
        assert_eq!(pref.source, SkillSource::AgentsMd);
        assert_eq!(pref.context, SkillContextMode::Inline);
        assert_eq!(
            pref.description.as_deref(),
            Some("Always use Rust-native solutions")
        );

        // Check Drizzle ORM skill
        let driz = skills.iter().find(|s| s.command == "drizzle-orm").unwrap();
        assert_eq!(driz.source, SkillSource::AgentsMd);
        assert_eq!(driz.context, SkillContextMode::Inline);
        assert!(driz.description.as_ref().unwrap().contains("TypeScript"));

        // Check Shadcn skill
        let shad = skills.iter().find(|s| s.command == "shadcn-ui").unwrap();
        assert_eq!(shad.context, SkillContextMode::Fork);
        assert_eq!(shad.permission_mode, Some(PermissionMode::Plan));
    }

    // === extract_file_references tests ===

    #[test]
    fn extracts_dot_slash_references() {
        let body = "Read ./implementer-prompt.md for details.\nAlso see ./subdir/helper.sh here.";
        let refs = extract_file_references(body);
        assert_eq!(refs, vec!["./implementer-prompt.md", "./subdir/helper.sh"]);
    }

    #[test]
    fn extracts_at_prefixed_references() {
        let body =
            "Use @testing-anti-patterns.md to avoid pitfalls.\nSee @graphviz-conventions.dot too.";
        let refs = extract_file_references(body);
        assert_eq!(
            refs,
            vec!["testing-anti-patterns.md", "graphviz-conventions.dot"]
        );
    }

    #[test]
    fn extracts_backtick_paths_with_directory() {
        let body = "Check `skills/brainstorming/visual-companion.md` for guidance.\nAlso `subdir/file.ts`.";
        let refs = extract_file_references(body);
        assert_eq!(
            refs,
            vec!["skills/brainstorming/visual-companion.md", "subdir/file.ts"]
        );
    }

    #[test]
    fn extracts_backtick_bare_filenames() {
        let body = "See `code-reviewer.md` for the template.\nUse `helper.sh` too.";
        let refs = extract_file_references(body);
        assert_eq!(refs, vec!["code-reviewer.md", "helper.sh"]);
    }

    #[test]
    fn excludes_well_known_files_in_backticks() {
        let body = "Check `CLAUDE.md` and `AGENTS.md` and `GEMINI.md` and `SKILL.md` and `README.md` and `package.json`.";
        let refs = extract_file_references(body);
        assert!(refs.is_empty());
    }

    #[test]
    fn excludes_urls_and_template_paths() {
        let body = "See https://example.com/file.md and path/to/test.md for examples.";
        let refs = extract_file_references(body);
        assert!(refs.is_empty());
    }

    #[test]
    fn deduplicates_preserving_order() {
        let body = "Use ./helper.md first.\nThen `helper.md` again.\nAnd @helper.md once more.";
        let refs = extract_file_references(body);
        assert_eq!(refs, vec!["./helper.md"]);
    }

    // === resolve_skill_reference tests ===

    #[test]
    fn resolves_reference_in_skill_directory() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("helper.md"), "helper content").unwrap();

        let result = resolve_skill_reference("./helper.md", &skill_dir);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), skill_dir.join("helper.md"));
    }

    #[test]
    fn resolves_reference_via_catalog_root_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("skills");
        let skill_dir = catalog.join("sdd");
        let other_skill = catalog.join("brainstorming");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::create_dir_all(&other_skill).unwrap();
        std::fs::write(other_skill.join("visual.md"), "visual content").unwrap();

        let result = resolve_skill_reference("brainstorming/visual.md", &skill_dir);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), other_skill.join("visual.md"));
    }

    #[test]
    fn resolves_skills_prefix_by_stripping() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("skills");
        let brainstorming = catalog.join("brainstorming");
        std::fs::create_dir_all(&brainstorming).unwrap();
        std::fs::write(brainstorming.join("visual.md"), "content").unwrap();

        let skill_dir = catalog.join("sdd");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let result = resolve_skill_reference("skills/brainstorming/visual.md", &skill_dir);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), brainstorming.join("visual.md"));
    }

    #[test]
    fn returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let result = resolve_skill_reference("./nonexistent.md", &skill_dir);
        assert!(result.is_none());
    }

    #[test]
    fn rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        // Create a file in the parent that should NOT be reachable
        std::fs::write(dir.path().join("secret.md"), "secret content").unwrap();

        let result = resolve_skill_reference("../secret.md", &skill_dir);
        assert!(result.is_none());
    }

    // === expanded_body tests ===

    #[test]
    fn expanded_body_inlines_referenced_files() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("helper.md"), "Helper content here.").unwrap();

        let skill = Skill {
            name: "test".into(),
            description: None,
            command: "test".into(),
            model: None,
            provider: None,
            permission_mode: None,
            context: SkillContextMode::Inline,
            agent: false,
            directory: skill_dir,
            body: "Main body.\n\nSee ./helper.md for details.".into(),
            source: SkillSource::FileSystem,
        };

        let expanded = skill.expanded_body();
        assert!(expanded.contains("Main body."));
        assert!(expanded.contains("===== helper.md ====="));
        assert!(expanded.contains("Helper content here."));
    }

    #[test]
    fn expanded_body_skips_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let skill = Skill {
            name: "test".into(),
            description: None,
            command: "test".into(),
            model: None,
            provider: None,
            permission_mode: None,
            context: SkillContextMode::Inline,
            agent: false,
            directory: skill_dir,
            body: "Main body.\n\nSee ./nonexistent.md for details.".into(),
            source: SkillSource::FileSystem,
        };

        let expanded = skill.expanded_body();
        assert_eq!(expanded, "Main body.\n\nSee ./nonexistent.md for details.");
    }

    #[test]
    fn expanded_body_inlines_at_prefixed_reference() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("tdd");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("testing-anti-patterns.md"),
            "Anti-pattern content.",
        )
        .unwrap();

        let skill = Skill {
            name: "test".into(),
            description: None,
            command: "test".into(),
            model: None,
            provider: None,
            permission_mode: None,
            context: SkillContextMode::Inline,
            agent: false,
            directory: skill_dir,
            body: "Main body.\n\nRead @testing-anti-patterns.md to avoid pitfalls.".into(),
            source: SkillSource::FileSystem,
        };

        let expanded = skill.expanded_body();
        assert!(expanded.contains("===== testing-anti-patterns.md ====="));
        assert!(expanded.contains("Anti-pattern content."));
    }

    #[test]
    fn expanded_body_bare_filename_resolves_only_in_skill_dir() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("skills");
        let skill_dir = catalog.join("review");
        let other_dir = catalog.join("other");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();
        std::fs::write(other_dir.join("template.md"), "Other content.").unwrap();

        let skill = Skill {
            name: "test".into(),
            description: None,
            command: "test".into(),
            model: None,
            provider: None,
            permission_mode: None,
            context: SkillContextMode::Inline,
            agent: false,
            directory: skill_dir,
            body: "See `template.md` for the template.".into(),
            source: SkillSource::FileSystem,
        };

        let expanded = skill.expanded_body();
        assert!(!expanded.contains("====="));
        assert!(!expanded.contains("Other content."));
    }

    #[test]
    fn expanded_body_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("helper.md"), "Helper.").unwrap();

        let skill = Skill {
            name: "test".into(),
            description: None,
            command: "test".into(),
            model: None,
            provider: None,
            permission_mode: None,
            context: SkillContextMode::Inline,
            agent: false,
            directory: skill_dir,
            body: "See ./helper.md and `helper.md` again.".into(),
            source: SkillSource::FileSystem,
        };

        let expanded = skill.expanded_body();
        let count = expanded.matches("===== helper.md =====").count();
        assert_eq!(count, 1);
    }
}
