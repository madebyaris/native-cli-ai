//! Tool that lets the LLM load a skill's full instructions by name.

use crate::skills::SkillCatalog;
use crate::tools::RecentSkillHints;
use crate::tools::ToolExecutor;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use std::path::PathBuf;

pub struct InvokeSkillTool {
    workspace_root: PathBuf,
    skill_directories: Vec<PathBuf>,
    recent_skills: RecentSkillHints,
}

impl InvokeSkillTool {
    pub fn new(
        workspace_root: PathBuf,
        skill_directories: Vec<PathBuf>,
        recent_skills: RecentSkillHints,
    ) -> Self {
        Self {
            workspace_root,
            skill_directories,
            recent_skills,
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for InvokeSkillTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "invoke_skill".into(),
            description: "Load a skill's full instructions by name. Use this when a task matches \
                an available skill from the skills manifest. Returns the complete skill \
                instructions to follow."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "skill_name": {
                        "type": "string",
                        "description": "The command name from the skills manifest (e.g., 'brainstorming', 'test-driven-development')"
                    }
                },
                "required": ["skill_name"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let skill_name = call.input["skill_name"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();

        if skill_name.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("skill_name is required".into()),
            };
        }

        let skills = match SkillCatalog::discover(&self.workspace_root, &self.skill_directories) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to discover skills: {e}")),
                };
            }
        };

        if let Some(skill) = skills.iter().find(|s| s.command == skill_name) {
            let body = skill.expanded_body();
            self.recent_skills.record(&skill.command);
            ToolResult {
                call_id: call.id.clone(),
                success: true,
                output: format!(
                    "Skill `{}` loaded. Follow these instructions:\n\n{}",
                    skill.command,
                    body.trim()
                ),
                error: None,
            }
        } else {
            let available: Vec<&str> = skills.iter().map(|s| s.command.as_str()).collect();
            ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Skill '{}' not found. Available skills: {}",
                    skill_name,
                    available.join(", ")
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(workspace: &std::path::Path) -> InvokeSkillTool {
        InvokeSkillTool::new(
            workspace.to_path_buf(),
            vec![std::path::PathBuf::from(".nca/skills")],
            RecentSkillHints::default(),
        )
    }

    fn make_call(skill_name: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "invoke_skill".into(),
            input: serde_json::json!({ "skill_name": skill_name }),
        }
    }

    #[test]
    fn definition_has_correct_name_and_parameters() {
        let dir = tempfile::tempdir().unwrap();
        let tool = make_tool(dir.path());
        let def = tool.definition();
        assert_eq!(def.name, "invoke_skill");
        assert!(def.description.contains("Load a skill"));
        assert!(def.parameters["properties"]["skill_name"].is_object());
        assert!(
            def.parameters["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("skill_name"))
        );
    }

    #[tokio::test]
    async fn returns_expanded_body_for_valid_skill() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".nca/skills/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: My Skill\ncommand: my-skill\ndescription: A test skill\n---\nDo the thing.\n\nSee ./helper.md for details.\n",
        )
        .unwrap();
        std::fs::write(skill_dir.join("helper.md"), "Helper content.").unwrap();

        let tool = make_tool(dir.path());
        let result = tool.execute(&make_call("my-skill")).await;

        assert!(result.success);
        assert!(result.output.contains("Skill `my-skill` loaded"));
        assert!(result.output.contains("Do the thing."));
        assert!(result.output.contains("===== helper.md ====="));
        assert!(result.output.contains("Helper content."));
    }

    #[tokio::test]
    async fn returns_error_for_unknown_skill() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".nca/skills/real-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Real\ncommand: real-skill\n---\nReal body.\n",
        )
        .unwrap();

        let tool = make_tool(dir.path());
        let result = tool.execute(&make_call("nonexistent")).await;

        assert!(!result.success);
        let err = result.error.unwrap();
        assert!(err.contains("not found"));
        assert!(err.contains("real-skill"));
    }

    #[tokio::test]
    async fn returns_error_for_empty_skill_name() {
        let dir = tempfile::tempdir().unwrap();
        let tool = make_tool(dir.path());
        let result = tool.execute(&make_call("")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("skill_name is required"));
    }

    #[tokio::test]
    async fn records_successfully_loaded_skill_for_follow_up_tools() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".nca/skills/review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Review\ncommand: review\n---\nReview code.\n",
        )
        .unwrap();

        let hints = RecentSkillHints::default();
        let tool = InvokeSkillTool::new(
            dir.path().to_path_buf(),
            vec![std::path::PathBuf::from(".nca/skills")],
            hints.clone(),
        );

        let result = tool.execute(&make_call("review")).await;

        assert!(result.success);
        assert_eq!(hints.recent(1), vec!["review"]);
    }
}
