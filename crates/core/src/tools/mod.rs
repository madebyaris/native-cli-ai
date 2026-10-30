#[cfg(target_os = "macos")]
pub mod app_input;
pub mod apply_patch;
pub mod ask_question;
pub mod bash;
pub mod code_intel_tool;
pub mod copy_path;
pub mod create_directory;
pub mod delete_path;
pub mod edit_file;
pub mod fetch_url;
pub mod filesystem;
pub mod git;
pub mod invoke_skill;
pub mod ledger;
pub mod list_directory;
pub mod mcp;
mod mcp_http;
pub mod move_path;
pub mod rename_path;
pub mod replace_match;
pub mod run_validation;
#[cfg(target_os = "macos")]
pub mod screen_record;
pub mod search;
pub mod show_image;
pub mod skill_hints;
pub mod spawn_subagent;
pub mod types;
pub mod update_todos;
pub mod web_search;
pub mod write_file;

pub use ask_question::AskQuestionTool;
pub use invoke_skill::InvokeSkillTool;
pub use skill_hints::RecentSkillHints;
pub use update_todos::{TodoStore, UpdateTodosTool, validate_todos};

use std::sync::Arc;

use nca_common::config::{RecordingConfig, WebConfig};
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};

use ledger::FileLedger;

/// Longest tool output, in characters, that reaches the model.
pub const MAX_TOOL_OUTPUT_CHARS: usize = 30_000;
/// Longest tool error, in characters, that reaches the model.
pub const MAX_TOOL_ERROR_CHARS: usize = 4_000;

/// Registry of available tools the agent can invoke.
pub struct ToolRegistry {
    tools: Vec<Box<dyn ToolExecutor>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Box<dyn ToolExecutor>) {
        self.tools.push(tool);
    }

    pub fn with_default_readonly_tools(
        workspace_root: std::path::PathBuf,
        web_config: WebConfig,
    ) -> Self {
        let ledger = Arc::new(FileLedger::new(workspace_root.clone()));
        Self::readonly_tools(workspace_root, web_config, ledger)
    }

    fn readonly_tools(
        workspace_root: std::path::PathBuf,
        web_config: WebConfig,
        ledger: Arc<FileLedger>,
    ) -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(filesystem::ReadFileTool::with_ledger(
            workspace_root.clone(),
            ledger,
        )));
        registry.register(Box::new(search::SearchCodeTool::new(
            workspace_root.clone(),
        )));
        registry.register(Box::new(list_directory::ListDirectoryTool::new(
            workspace_root.clone(),
        )));
        registry.register(Box::new(git::GitStatusTool::new(workspace_root.clone())));
        registry.register(Box::new(git::GitDiffTool::new(workspace_root)));
        registry.register(Box::new(web_search::WebSearchTool::new(web_config.clone())));
        registry.register(Box::new(fetch_url::FetchUrlTool::new(web_config)));
        registry
    }

    pub fn with_default_full_tools(
        workspace_root: std::path::PathBuf,
        web_config: WebConfig,
        recording: RecordingConfig,
    ) -> Self {
        // Only the macOS screen recorder reads the recording config.
        #[cfg(not(target_os = "macos"))]
        let _ = recording;
        // One ledger per registry: a read by any tool unlocks edits by the others.
        let ledger = Arc::new(FileLedger::new(workspace_root.clone()));
        let mut registry = Self::readonly_tools(workspace_root.clone(), web_config, ledger.clone());
        registry.register(Box::new(code_intel_tool::CodeIntelTool::new(
            crate::code_intel::FastLocalCodeIntel::new(workspace_root.clone()),
        )));
        registry.register(Box::new(write_file::WriteFileTool::with_ledger(
            workspace_root.clone(),
            ledger.clone(),
        )));
        registry.register(Box::new(create_directory::CreateDirectoryTool::new(
            workspace_root.clone(),
        )));
        registry.register(Box::new(apply_patch::ApplyPatchTool::with_ledger(
            workspace_root.clone(),
            ledger.clone(),
        )));
        registry.register(Box::new(edit_file::EditFileTool::with_ledger(
            workspace_root.clone(),
            ledger.clone(),
        )));
        registry.register(Box::new(replace_match::ReplaceMatchTool::with_ledger(
            workspace_root.clone(),
            ledger.clone(),
        )));
        registry.register(Box::new(rename_path::RenamePathTool::with_ledger(
            workspace_root.clone(),
            ledger.clone(),
        )));
        registry.register(Box::new(move_path::MovePathTool::with_ledger(
            workspace_root.clone(),
            ledger.clone(),
        )));
        registry.register(Box::new(copy_path::CopyPathTool::with_ledger(
            workspace_root.clone(),
            ledger.clone(),
        )));
        registry.register(Box::new(delete_path::DeletePathTool::with_ledger(
            workspace_root.clone(),
            ledger,
        )));
        #[cfg(target_os = "macos")]
        registry.register(Box::new(screen_record::ScreenRecordTool::new(
            nca_common::config::resolve_recordings_dir(&recording.dir, &workspace_root)
                .map_err(|error| error.to_string()),
        )));
        registry.register(Box::new(show_image::ShowImageTool::new(
            workspace_root.clone(),
        )));
        registry.register(Box::new(run_validation::RunValidationTool::new(
            workspace_root,
        )));
        #[cfg(target_os = "macos")]
        registry.register(Box::new(app_input::AppInputTool));
        registry
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    /// Run the named tool. Its output and error are capped at
    /// `MAX_TOOL_OUTPUT_CHARS` and `MAX_TOOL_ERROR_CHARS`.
    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        for tool in &self.tools {
            if tool.definition().name == call.name {
                return cap_result(tool.execute(call).await);
            }
        }

        ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: String::new(),
            error: Some(format!("Unknown tool: {}", call.name)),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait implemented by each tool.
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, call: &ToolCall) -> ToolResult;
}

fn cap_result(mut result: ToolResult) -> ToolResult {
    result.output = cap_text(result.output, MAX_TOOL_OUTPUT_CHARS);
    result.error = result
        .error
        .map(|error| cap_text(error, MAX_TOOL_ERROR_CHARS));
    result
}

/// Bound `text` to `max` characters, marker included. Over the cap it keeps
/// the first 75% and the last 25% of the budget and notes what was cut.
/// Cuts land on char boundaries.
pub(crate) fn cap_text(text: String, max: usize) -> String {
    // Bytes bound chars, so short text skips the char scan.
    if text.len() <= max {
        return text;
    }
    let total = text.chars().count();
    if total <= max {
        return text;
    }

    // Reserve room for the marker, sized for the largest possible omitted count.
    let marker_len = omission_marker(total, max).chars().count();
    let keep = max.saturating_sub(marker_len);
    let head_chars = keep * 3 / 4;
    let tail_chars = keep - head_chars;
    let omitted = total - head_chars - tail_chars;

    let head_end = text
        .char_indices()
        .nth(head_chars)
        .map_or(text.len(), |(index, _)| index);
    let tail_start = match tail_chars {
        0 => text.len(),
        n => text
            .char_indices()
            .rev()
            .nth(n - 1)
            .map_or(0, |(index, _)| index),
    };

    let marker = omission_marker(omitted, max);
    let mut capped = String::with_capacity(head_end + marker.len() + (text.len() - tail_start));
    capped.push_str(&text[..head_end]);
    capped.push_str(&marker);
    capped.push_str(&text[tail_start..]);
    capped
}

fn omission_marker(omitted: usize, max: usize) -> String {
    format!(
        "\n[... {omitted} characters omitted. Tool output was capped at {max} characters; narrow the request with start_line/end_line, a more specific search, or head/tail in a shell command ...]\n"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: name.into(),
            input,
        }
    }

    fn full_registry(dir: &tempfile::TempDir) -> ToolRegistry {
        // The supervisor passes a canonical root; copy, rename and delete
        // compare paths against it without canonicalizing it themselves.
        let root = dir.path().canonicalize().unwrap();
        ToolRegistry::with_default_full_tools(
            root,
            WebConfig::default(),
            RecordingConfig::default(),
        )
    }

    /// The macOS tools are registered, and the registry runs them through their own argument
    /// checks. Both calls below fail before touching the screen.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_tools_are_registered_and_dispatch_through_the_registry() {
        let dir = tempfile::tempdir().expect("temp dir");
        let registry = full_registry(&dir);
        let names: Vec<String> = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        assert!(names.iter().any(|name| name == "app_input"));
        assert!(names.iter().any(|name| name == "screen_record"));

        let stop = nca_common::tool::ToolCall {
            id: "call-stop".into(),
            name: "screen_record".into(),
            input: serde_json::json!({"action": "stop", "id": "rec-9"}),
        };
        let result = registry.execute(&stop).await;
        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("No active recording")
        );

        let press = nca_common::tool::ToolCall {
            id: "call-press".into(),
            name: "app_input".into(),
            input: serde_json::json!({"action": "press"}),
        };
        let result = registry.execute(&press).await;
        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("`app`"));
    }

    fn read_input(path: &str) -> serde_json::Value {
        json!({ "path": path })
    }

    fn edit_input(path: &str, old_text: &str, new_text: &str) -> serde_json::Value {
        json!({ "path": path, "old_text": old_text, "new_text": new_text })
    }

    /// Splits capped text into (head, omitted count, tail).
    fn split_marker(capped: &str, max: usize) -> (&str, usize, &str) {
        let start = capped.find("\n[... ").expect("marker start");
        let rest = &capped[start + "\n[... ".len()..];
        let digits = rest.split(' ').next().expect("omitted count");
        let omitted = digits.parse().expect("numeric omitted count");
        let tail_start =
            capped.find("shell command ...]\n").expect("marker end") + "shell command ...]\n".len();
        assert!(capped.contains(&format!(
            "characters omitted. Tool output was capped at {max} characters; narrow the request with start_line/end_line"
        )));
        (&capped[..start], omitted, &capped[tail_start..])
    }

    struct FixedTool {
        output: String,
        error: Option<String>,
    }

    #[async_trait::async_trait]
    impl ToolExecutor for FixedTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "fixed".into(),
                description: "returns fixed text".into(),
                parameters: json!({ "type": "object", "properties": {} }),
            }
        }

        async fn execute(&self, call: &ToolCall) -> ToolResult {
            ToolResult {
                call_id: call.id.clone(),
                success: self.error.is_none(),
                output: self.output.clone(),
                error: self.error.clone(),
            }
        }
    }

    #[test]
    fn cap_text_leaves_text_within_the_cap_unchanged() {
        let exact = "x".repeat(1_000);
        assert_eq!(cap_text(exact.clone(), 1_000), exact);
        assert_eq!(cap_text(String::new(), 1_000), "");
        // Multi-byte text counts characters, not bytes.
        let wide = "日".repeat(1_000);
        assert_eq!(cap_text(wide.clone(), 1_000), wide);
    }

    #[test]
    fn cap_text_keeps_head_and_tail_around_a_marker() {
        let text: String = (0..100_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();

        let capped = cap_text(text.clone(), MAX_TOOL_OUTPUT_CHARS);

        assert!(capped.chars().count() <= MAX_TOOL_OUTPUT_CHARS);
        let (head, omitted, tail) = split_marker(&capped, MAX_TOOL_OUTPUT_CHARS);
        assert_eq!(
            head.chars().count() + tail.chars().count() + omitted,
            100_000
        );
        assert!(text.starts_with(head));
        assert!(text.ends_with(tail));
        // Roughly three quarters head, one quarter tail.
        let ratio =
            head.chars().count() as f64 / (head.chars().count() + tail.chars().count()) as f64;
        assert!((0.74..=0.76).contains(&ratio), "head ratio {ratio}");
    }

    #[test]
    fn cap_text_cuts_one_character_over_the_cap() {
        let capped = cap_text("y".repeat(1_001), 1_000);

        assert!(capped.chars().count() <= 1_000);
        assert!(capped.contains("characters omitted"));
    }

    #[test]
    fn cap_text_does_not_split_multibyte_characters() {
        for unit in ["é", "日", "😀", "a日é😀"] {
            for extra in [1, 2, 3, 7, 1_001] {
                let text = unit.repeat(1_000 + extra);
                let capped = cap_text(text.clone(), 1_000);

                assert!(capped.chars().count() <= 1_000, "{unit} +{extra}");
                let (head, omitted, tail) = split_marker(&capped, 1_000);
                assert_eq!(
                    head.chars().count() + tail.chars().count() + omitted,
                    text.chars().count()
                );
                assert!(text.starts_with(head));
                assert!(text.ends_with(tail));
            }
        }
    }

    #[test]
    fn cap_text_survives_a_cap_smaller_than_the_marker() {
        let capped = cap_text("😀".repeat(500), 10);

        assert!(capped.contains("characters omitted"));
    }

    #[tokio::test]
    async fn registry_caps_output_and_error_separately() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(FixedTool {
            output: "o".repeat(MAX_TOOL_OUTPUT_CHARS * 2),
            error: Some("e".repeat(MAX_TOOL_ERROR_CHARS * 3)),
        }));

        let result = registry.execute(&call("fixed", json!({}))).await;

        assert!(!result.success);
        assert!(result.output.chars().count() <= MAX_TOOL_OUTPUT_CHARS);
        assert!(result.output.contains("characters omitted"));
        let error = result.error.expect("error kept");
        assert!(error.chars().count() <= MAX_TOOL_ERROR_CHARS);
        assert!(error.contains(&format!("capped at {MAX_TOOL_ERROR_CHARS} characters")));
    }

    #[tokio::test]
    async fn registry_leaves_small_results_unchanged() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(FixedTool {
            output: "small output".into(),
            error: None,
        }));

        let result = registry.execute(&call("fixed", json!({}))).await;

        assert!(result.success);
        assert_eq!(result.output, "small output");
        assert_eq!(result.error, None);
    }

    #[tokio::test]
    async fn paging_through_a_wide_file_never_hits_the_output_cap() {
        let dir = tempfile::tempdir().unwrap();
        // 1500 lines of 80 chars: more than fits under the cap in one read.
        let content: String = (1..=1500).map(|n| format!("{n:0>79}\n")).collect();
        std::fs::write(dir.path().join("wide.txt"), content).unwrap();
        let registry = full_registry(&dir);

        let mut next = 1;
        let mut pages = 0;
        while next <= 1500 {
            let result = registry
                .execute(&call(
                    "read_file",
                    json!({ "path": "wide.txt", "start_line": next }),
                ))
                .await;
            assert!(result.success, "{result:?}");
            assert!(!result.output.contains("characters omitted"));
            let (header, body) = result.output.split_once('\n').unwrap();
            let range = header
                .strip_prefix("[showing lines ")
                .and_then(|rest| rest.split(' ').next())
                .unwrap();
            let (first, last) = range.split_once('-').unwrap();
            assert_eq!(first.parse::<usize>().unwrap(), next, "{header}");
            assert_eq!(body[..79].parse::<usize>().unwrap(), next);
            next = last.parse::<usize>().unwrap() + 1;
            pages += 1;
        }

        assert!(pages >= 4, "{pages} pages");
    }

    #[tokio::test]
    async fn editing_an_unread_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\n").unwrap();
        let registry = full_registry(&dir);

        let edit = registry
            .execute(&call("edit_file", edit_input("a.txt", "alpha", "beta")))
            .await;
        let patch = registry
            .execute(&call(
                "apply_patch",
                json!({ "path": "a.txt", "edits": [{ "old_text": "alpha", "new_text": "beta" }] }),
            ))
            .await;

        for result in [edit, patch] {
            assert!(!result.success);
            let error = result.error.unwrap();
            assert!(error.contains("read_file"), "{error}");
            assert!(
                error.contains("`a.txt` has not been read in this session"),
                "{error}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "alpha\n"
        );
    }

    #[tokio::test]
    async fn reading_then_editing_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let registry = full_registry(&dir);

        let read = registry
            .execute(&call("read_file", read_input("a.txt")))
            .await;
        assert!(read.success, "{read:?}");
        let edit = registry
            .execute(&call("edit_file", edit_input("a.txt", "alpha", "ALPHA")))
            .await;
        assert!(edit.success, "{edit:?}");
        // The edit refreshed the ledger, so a second edit needs no new read.
        let again = registry
            .execute(&call("edit_file", edit_input("a.txt", "beta", "BETA")))
            .await;
        assert!(again.success, "{again:?}");
        let patch = registry
            .execute(&call(
                "apply_patch",
                json!({ "path": "a.txt", "edits": [{ "old_text": "gamma", "new_text": "GAMMA" }] }),
            ))
            .await;
        assert!(patch.success, "{patch:?}");

        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ALPHA\nBETA\nGAMMA\n"
        );
    }

    #[tokio::test]
    async fn a_ranged_read_unlocks_editing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let registry = full_registry(&dir);

        let read = registry
            .execute(&call(
                "read_file",
                json!({ "path": "a.txt", "start_line": 2, "end_line": 2 }),
            ))
            .await;
        assert!(read.success, "{read:?}");
        let edit = registry
            .execute(&call("edit_file", edit_input("a.txt", "three", "3")))
            .await;

        assert!(edit.success, "{edit:?}");
    }

    #[tokio::test]
    async fn editing_after_the_file_changed_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let registry = full_registry(&dir);

        let read = registry
            .execute(&call("read_file", read_input("a.txt")))
            .await;
        assert!(read.success, "{read:?}");
        std::fs::write(&path, "alpha changed elsewhere\n").unwrap();
        let edit = registry
            .execute(&call("edit_file", edit_input("a.txt", "alpha", "beta")))
            .await;

        assert!(!edit.success);
        let error = edit.error.unwrap();
        assert!(
            error.contains("`a.txt` changed since it was last read"),
            "{error}"
        );
        assert!(error.contains("read_file"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "alpha changed elsewhere\n"
        );

        // Reading again unlocks the file.
        let reread = registry
            .execute(&call("read_file", read_input("a.txt")))
            .await;
        assert!(reread.success, "{reread:?}");
        let retry = registry
            .execute(&call("edit_file", edit_input("a.txt", "alpha", "beta")))
            .await;
        assert!(retry.success, "{retry:?}");
    }

    #[tokio::test]
    async fn writing_a_new_file_needs_no_read_and_can_be_edited_after() {
        let dir = tempfile::tempdir().unwrap();
        let registry = full_registry(&dir);

        let write = registry
            .execute(&call(
                "write_file",
                json!({ "path": "nested/new.txt", "content": "fresh\n" }),
            ))
            .await;
        assert!(write.success, "{write:?}");
        let edit = registry
            .execute(&call(
                "edit_file",
                edit_input("nested/new.txt", "fresh", "edited"),
            ))
            .await;

        assert!(edit.success, "{edit:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("nested/new.txt")).unwrap(),
            "edited\n"
        );
    }

    #[tokio::test]
    async fn overwriting_an_unread_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "precious\n").unwrap();
        let registry = full_registry(&dir);

        let write = registry
            .execute(&call(
                "write_file",
                json!({ "path": "a.txt", "content": "clobbered\n" }),
            ))
            .await;

        assert!(!write.success);
        let error = write.error.unwrap();
        assert!(error.contains("read_file"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "precious\n");

        // After a read the overwrite goes through.
        let read = registry
            .execute(&call("read_file", read_input("a.txt")))
            .await;
        assert!(read.success, "{read:?}");
        let retry = registry
            .execute(&call(
                "write_file",
                json!({ "path": "a.txt", "content": "replaced\n" }),
            ))
            .await;
        assert!(retry.success, "{retry:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replaced\n");
    }

    #[tokio::test]
    async fn replace_match_keeps_an_earlier_read_valid() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha beta\n").unwrap();
        let registry = full_registry(&dir);

        let read = registry
            .execute(&call("read_file", read_input("a.txt")))
            .await;
        assert!(read.success, "{read:?}");
        let replace = registry
            .execute(&call(
                "replace_match",
                json!({
                    "path": "a.txt",
                    "line": 1,
                    "column": 1,
                    "old_text": "alpha",
                    "new_text": "ALPHA"
                }),
            ))
            .await;
        assert!(replace.success, "{replace:?}");
        let edit = registry
            .execute(&call("edit_file", edit_input("a.txt", "beta", "BETA")))
            .await;

        assert!(edit.success, "{edit:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ALPHA BETA\n"
        );
    }

    #[tokio::test]
    async fn replace_match_does_not_need_a_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\n").unwrap();
        let registry = full_registry(&dir);

        let replace = registry
            .execute(&call(
                "replace_match",
                json!({
                    "path": "a.txt",
                    "line": 1,
                    "column": 1,
                    "old_text": "alpha",
                    "new_text": "beta"
                }),
            ))
            .await;

        assert!(replace.success, "{replace:?}");
    }

    #[tokio::test]
    async fn copy_over_a_read_file_keeps_it_editable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("src.txt"), "from source\n").unwrap();
        std::fs::write(dir.path().join("dst.txt"), "old destination\n").unwrap();
        let registry = full_registry(&dir);

        let read = registry
            .execute(&call("read_file", read_input("dst.txt")))
            .await;
        assert!(read.success, "{read:?}");
        let copy = registry
            .execute(&call(
                "copy_path",
                json!({ "from": "src.txt", "to": "dst.txt" }),
            ))
            .await;
        assert!(copy.success, "{copy:?}");
        let edit = registry
            .execute(&call("edit_file", edit_input("dst.txt", "source", "copy")))
            .await;

        assert!(edit.success, "{edit:?}");
    }

    #[tokio::test]
    async fn rename_and_move_forget_the_old_path() {
        for tool in ["rename_path", "move_path"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("old.txt");
            std::fs::write(&path, "alpha\n").unwrap();
            let registry = full_registry(&dir);

            let read = registry
                .execute(&call("read_file", read_input("old.txt")))
                .await;
            assert!(read.success, "{read:?}");
            let moved = registry
                .execute(&call(tool, json!({ "from": "old.txt", "to": "new.txt" })))
                .await;
            assert!(moved.success, "{tool}: {moved:?}");
            // A different file now sits at the old path; the earlier read must not cover it.
            std::fs::write(&path, "alpha\n").unwrap();
            let edit = registry
                .execute(&call("edit_file", edit_input("old.txt", "alpha", "beta")))
                .await;

            assert!(!edit.success, "{tool}");
            assert!(edit.error.unwrap().contains("has not been read"), "{tool}");
        }
    }

    #[tokio::test]
    async fn deleting_forgets_files_and_directories_that_were_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
        std::fs::write(dir.path().join("pkg/a.txt"), "alpha\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "alpha\n").unwrap();
        let registry = full_registry(&dir);
        for path in ["pkg/a.txt", "b.txt"] {
            let read = registry.execute(&call("read_file", read_input(path))).await;
            assert!(read.success, "{read:?}");
        }

        for target in ["pkg", "b.txt"] {
            let delete = registry
                .execute(&call(
                    "delete_path",
                    json!({ "path": target, "recursive": true }),
                ))
                .await;
            assert!(delete.success, "{delete:?}");
        }
        // Recreate identical content outside the tools; the old reads must not cover it.
        std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
        std::fs::write(dir.path().join("pkg/a.txt"), "alpha\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "alpha\n").unwrap();

        for path in ["pkg/a.txt", "b.txt"] {
            let edit = registry
                .execute(&call("edit_file", edit_input(path, "alpha", "beta")))
                .await;
            assert!(!edit.success, "{path}");
            assert!(edit.error.unwrap().contains("has not been read"), "{path}");
        }
    }

    #[tokio::test]
    async fn each_registry_has_its_own_ledger() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\n").unwrap();
        let first = full_registry(&dir);
        let second = full_registry(&dir);

        let read = first.execute(&call("read_file", read_input("a.txt"))).await;
        assert!(read.success, "{read:?}");
        let edit = second
            .execute(&call("edit_file", edit_input("a.txt", "alpha", "beta")))
            .await;

        assert!(!edit.success);
    }
}
