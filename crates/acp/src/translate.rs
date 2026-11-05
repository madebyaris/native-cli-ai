//! Translate nca's `AgentEvent` stream into ACP v1 session updates.

use agent_client_protocol::schema::v1::{
    CompactionStatus, CompactionUpdate, ContentBlock, ContentChunk, Cost, ImageContent, Plan,
    PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionUpdate, ToolCall, ToolCallContent,
    ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind, UsageUpdate,
};
use nca_common::todo::{AgentTodo, TodoStatus};
use nca_common::tool::ToolResult;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Tool output is echoed to the client for display. Keep one pathological
/// result from pinning megabytes in the client and in our JSON buffer.
const MAX_TOOL_OUTPUT_CHARS: usize = 16_000;
const MAX_TITLE_CHARS: usize = 160;

pub fn text_chunk(text: impl Into<String>) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(text.into())))
}

pub fn thought_chunk(text: impl Into<String>) -> SessionUpdate {
    SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from(text.into())))
}

pub fn user_chunk(text: impl Into<String>) -> SessionUpdate {
    SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::from(text.into())))
}

pub fn tool_kind(name: &str) -> ToolKind {
    match name {
        "read_file" | "list_directory" | "git_status" | "git_diff" => ToolKind::Read,
        "write_file" | "edit_file" | "apply_patch" | "replace_match" | "create_directory" => {
            ToolKind::Edit
        }
        "delete_path" => ToolKind::Delete,
        "move_path" | "rename_path" | "copy_path" => ToolKind::Move,
        "search_code" | "query_symbols" => ToolKind::Search,
        "execute_bash" | "run_validation" => ToolKind::Execute,
        "web_search" | "fetch_url" => ToolKind::Fetch,
        "update_todos" => ToolKind::Think,
        _ => ToolKind::Other,
    }
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max).collect();
    out.push_str("… [truncated]");
    out
}

pub fn tool_title(name: &str, input: &Value) -> String {
    let detail = nca_core::approval::extract_meaningful_text(input);
    let title = if detail.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {detail}")
    };
    truncate_chars(&title, MAX_TITLE_CHARS)
}

/// Files a tool touches, so clients can offer follow-along navigation.
pub fn tool_locations(name: &str, input: &Value, workspace: &Path) -> Vec<ToolCallLocation> {
    if !matches!(
        tool_kind(name),
        ToolKind::Read | ToolKind::Edit | ToolKind::Delete | ToolKind::Move
    ) {
        return Vec::new();
    }
    let mut paths = Vec::new();
    if let Value::Object(map) = input {
        for key in ["path", "file_path", "source", "destination"] {
            if let Some(Value::String(path)) = map.get(key) {
                paths.push(path.clone());
            }
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let absolute = PathBuf::from(&path);
            let absolute = if absolute.is_absolute() {
                absolute
            } else {
                workspace.join(absolute)
            };
            ToolCallLocation::new(absolute)
        })
        .collect()
}

pub fn tool_started(call_id: &str, tool: &str, input: &Value, workspace: &Path) -> SessionUpdate {
    let call = ToolCall::new(call_id.to_string(), tool_title(tool, input))
        .name(Some(tool.to_string()))
        .kind(tool_kind(tool))
        .status(ToolCallStatus::InProgress)
        .locations(tool_locations(tool, input, workspace))
        .raw_input(Some(input.clone()));
    SessionUpdate::ToolCall(call)
}

pub fn tool_completed(call_id: &str, tool: &str, result: &ToolResult) -> SessionUpdate {
    tool_completed_with_image(call_id, tool, result, None)
}

/// Like [`tool_completed`], with an image shown under the tool's text output.
pub fn tool_completed_with_image(
    call_id: &str,
    tool: &str,
    result: &ToolResult,
    image: Option<ContentBlock>,
) -> SessionUpdate {
    let status = if result.success {
        ToolCallStatus::Completed
    } else {
        ToolCallStatus::Failed
    };
    let body = if result.success {
        result.output.clone()
    } else {
        match &result.error {
            Some(error) if result.output.is_empty() => error.clone(),
            Some(error) => format!("{}\n\n{error}", result.output),
            None => result.output.clone(),
        }
    };
    let body = truncate_chars(&body, MAX_TOOL_OUTPUT_CHARS);
    let mut content = vec![ToolCallContent::from(ContentBlock::from(body))];
    content.extend(image.map(ToolCallContent::from));
    let mut fields = ToolCallUpdateFields::new()
        .status(status)
        .content(Some(content))
        .raw_output(Some(serde_json::json!({
            "success": result.success,
            "output": truncate_chars(&result.output, MAX_TOOL_OUTPUT_CHARS),
            "error": result.error.as_deref().map(|e| truncate_chars(e, MAX_TOOL_OUTPUT_CHARS)),
        })));
    if !tool.is_empty() {
        fields = fields.name(Some(tool.to_string()));
    }
    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(call_id.to_string(), fields))
}

pub fn plan_update(todos: &[AgentTodo]) -> SessionUpdate {
    let entries: Vec<PlanEntry> = todos
        .iter()
        .filter(|todo| todo.status != TodoStatus::Cancelled)
        .enumerate()
        .map(|(index, todo)| {
            let status = match todo.status {
                TodoStatus::InProgress => PlanEntryStatus::InProgress,
                TodoStatus::Completed => PlanEntryStatus::Completed,
                TodoStatus::Pending | TodoStatus::Cancelled => PlanEntryStatus::Pending,
            };
            let priority = if index == 0 {
                PlanEntryPriority::High
            } else if index + 1 == todos.len() {
                PlanEntryPriority::Low
            } else {
                PlanEntryPriority::Medium
            };
            PlanEntry::new(todo.content.clone(), priority, status)
        })
        .collect();
    SessionUpdate::Plan(Plan::new(entries))
}

pub fn usage(used: u64, size: u64, cost_usd: f64) -> SessionUpdate {
    let mut update = UsageUpdate::new(used, size);
    if cost_usd > 0.0 {
        update = update.cost(Some(Cost::new(cost_usd, "USD")));
    }
    SessionUpdate::UsageUpdate(update)
}

/// A context-compaction lifecycle update. Only send this when the client
/// advertised the `session.compaction` capability.
pub fn compaction_update(
    compaction_id: &str,
    status: CompactionStatus,
    detail: Option<&str>,
) -> SessionUpdate {
    let mut update = CompactionUpdate::new(compaction_id.to_string(), status.clone());
    if let Some(detail) = detail {
        if status == CompactionStatus::Completed {
            update = update.summary(vec![ContentBlock::from(detail.to_string())]);
        } else if status == CompactionStatus::Failed {
            update = update.error(detail.to_string());
        }
    }
    SessionUpdate::CompactionUpdate(update)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::todo::AgentTodo;
    use serde_json::json;

    fn result(success: bool, output: &str, error: Option<&str>) -> ToolResult {
        ToolResult {
            call_id: String::new(),
            success,
            output: output.to_string(),
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn tool_kinds_cover_the_shipped_toolset() {
        assert_eq!(tool_kind("read_file"), ToolKind::Read);
        assert_eq!(tool_kind("edit_file"), ToolKind::Edit);
        assert_eq!(tool_kind("delete_path"), ToolKind::Delete);
        assert_eq!(tool_kind("move_path"), ToolKind::Move);
        assert_eq!(tool_kind("search_code"), ToolKind::Search);
        assert_eq!(tool_kind("execute_bash"), ToolKind::Execute);
        assert_eq!(tool_kind("web_search"), ToolKind::Fetch);
        assert_eq!(tool_kind("unknown_tool"), ToolKind::Other);
    }

    #[test]
    fn tool_locations_resolve_relative_paths_only_for_file_tools() {
        let workspace = Path::new("/work");
        let locations = tool_locations("edit_file", &json!({"path": "src/main.rs"}), workspace);
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].path, PathBuf::from("/work/src/main.rs"));

        let absolute = tool_locations("read_file", &json!({"path": "/etc/hosts"}), workspace);
        assert_eq!(absolute[0].path, PathBuf::from("/etc/hosts"));

        assert!(tool_locations("execute_bash", &json!({"path": "x"}), workspace).is_empty());
    }

    #[test]
    fn tool_titles_are_bounded() {
        let title = tool_title("read_file", &json!({"path": "a".repeat(500)}));
        assert!(title.chars().count() <= MAX_TITLE_CHARS + 14);
        assert!(title.ends_with("[truncated]"));
    }

    #[test]
    fn tool_updates_serialize_to_the_protocol_shape() {
        let started = serde_json::to_value(tool_started(
            "call-1",
            "read_file",
            &json!({"path": "src/lib.rs"}),
            Path::new("/work"),
        ))
        .expect("serialize");
        assert_eq!(started["sessionUpdate"], "tool_call");
        assert_eq!(started["toolCallId"], "call-1");
        assert_eq!(started["status"], "in_progress");
        assert_eq!(started["kind"], "read");
        assert_eq!(started["locations"][0]["path"], "/work/src/lib.rs");

        let completed =
            serde_json::to_value(tool_completed("call-1", "", &result(true, "ok", None)))
                .expect("serialize");
        assert_eq!(completed["sessionUpdate"], "tool_call_update");
        assert_eq!(completed["status"], "completed");
        assert!(completed.get("name").is_none());
        assert_eq!(completed["rawOutput"]["success"], true);
    }

    #[test]
    fn failed_tool_updates_keep_the_error_visible() {
        let update = tool_completed("call-2", "execute_bash", &result(false, "", Some("boom")));
        let value = serde_json::to_value(update).expect("serialize");
        assert_eq!(value["status"], "failed");
        let text = value["content"][0]["content"]["text"]
            .as_str()
            .expect("content text");
        assert!(text.contains("boom"));
    }

    #[test]
    fn tool_output_is_truncated_before_it_reaches_the_client() {
        let huge = "x".repeat(MAX_TOOL_OUTPUT_CHARS * 2);
        let value = serde_json::to_value(tool_completed(
            "call-3",
            "read_file",
            &result(true, &huge, None),
        ))
        .expect("serialize");
        let text = value["content"][0]["content"]["text"]
            .as_str()
            .expect("text");
        assert!(text.chars().count() < MAX_TOOL_OUTPUT_CHARS + 32);
        assert!(text.ends_with("[truncated]"));
    }

    #[test]
    fn plan_updates_skip_cancelled_entries() {
        let todos = vec![
            AgentTodo {
                id: "1".into(),
                content: "first".into(),
                status: TodoStatus::Completed,
                source: None,
            },
            AgentTodo {
                id: "2".into(),
                content: "gone".into(),
                status: TodoStatus::Cancelled,
                source: None,
            },
            AgentTodo {
                id: "3".into(),
                content: "next".into(),
                status: TodoStatus::InProgress,
                source: None,
            },
        ];
        let value = serde_json::to_value(plan_update(&todos)).expect("serialize");
        assert_eq!(value["sessionUpdate"], "plan");
        let entries = value["entries"].as_array().expect("entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1]["content"], "next");
        assert_eq!(entries[1]["status"], "in_progress");
    }

    #[test]
    fn usage_carries_cost_only_when_it_is_known() {
        let plain = serde_json::to_value(usage(1000, 200_000, 0.0)).expect("serialize");
        assert_eq!(plain["used"], 1000);
        assert_eq!(plain["size"], 200_000);
        assert!(plain.get("cost").is_none());

        let priced = serde_json::to_value(usage(1000, 200_000, 0.25)).expect("serialize");
        assert_eq!(priced["cost"]["amount"], 0.25);
        assert_eq!(priced["cost"]["currency"], "USD");
    }

    #[test]
    fn compaction_updates_carry_status_and_detail() {
        let started = serde_json::to_value(compaction_update(
            "ctx-1",
            CompactionStatus::InProgress,
            Some("summarizing"),
        ))
        .expect("serialize");
        assert_eq!(started["sessionUpdate"], "compaction_update");
        assert_eq!(started["compactionId"], "ctx-1");
        assert_eq!(started["status"], "in_progress");

        let done = serde_json::to_value(compaction_update(
            "ctx-1",
            CompactionStatus::Completed,
            Some("older context summarized"),
        ))
        .expect("serialize");
        assert_eq!(done["status"], "completed");
        assert_eq!(done["summary"][0]["text"], "older context summarized");
    }
}

/// Send `show_image` pictures as a Markdown image line instead of an ACP
/// image block. Set once per process from the client's name: T3 Code turns
/// image blocks into an `[ACP image …]` placeholder but renders Markdown
/// images with absolute file paths. One `nca acp` process serves one client.
static IMAGES_AS_MARKDOWN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Pick the image format for the connected client.
pub fn set_client_name(name: &str) {
    let name = name.to_ascii_lowercase();
    let markdown = name.starts_with("t3");
    IMAGES_AS_MARKDOWN.store(markdown, std::sync::atomic::Ordering::Relaxed);
}

fn images_as_markdown() -> bool {
    IMAGES_AS_MARKDOWN.load(std::sync::atomic::Ordering::Relaxed)
}

/// A Markdown image line for clients that render Markdown but not image
/// blocks. Blank lines keep it apart from the surrounding reply text.
fn markdown_image(path: &Path) -> String {
    let alt = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("image")
        .replace(['[', ']'], "");
    let target = path.display().to_string().replace(' ', "%20");
    format!("\n\n![{alt}]({target})\n\n")
}

/// How a tool's image should reach the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolImage {
    /// `show_image`: shown in the chat as part of the agent's reply.
    Message(PathBuf),
    /// A screenshot: attached to the tool call's output.
    ToolOutput(PathBuf),
}

/// The image a successful tool call produced, if any.
pub fn tool_image(
    tool: &str,
    input: &Value,
    result: &ToolResult,
    workspace: &Path,
) -> Option<ToolImage> {
    if !result.success {
        return None;
    }
    match tool {
        "show_image" => input.get("path").and_then(Value::as_str).map(|raw| {
            ToolImage::Message(nca_core::tools::show_image::resolve_image_path(
                raw, workspace,
            ))
        }),
        "screen_record" if input.get("action").and_then(Value::as_str) == Some("screenshot") => {
            screenshot_path(&result.output).map(ToolImage::ToolOutput)
        }
        _ => None,
    }
}

/// The `.png` path in a screenshot result ("Saved a screenshot of … to <path>. …").
fn screenshot_path(output: &str) -> Option<PathBuf> {
    let end = output.find(".png")? + ".png".len();
    let start = output[..end].rfind(" to ").map(|i| i + " to ".len())?;
    Some(PathBuf::from(&output[start..end]))
}

/// Read an image file into an ACP image block. `None` when it is missing,
/// not an image, or over the size limit; the text result still names it.
pub async fn image_block(path: &Path) -> Option<ContentBlock> {
    use base64::Engine;
    use nca_core::tools::show_image::{MAX_IMAGE_BYTES, image_mime};
    let mime = image_mime(path)?;
    let meta = tokio::fs::metadata(path).await.ok()?;
    if !meta.is_file() || meta.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let uri = format!("file://{}", path.display());
    Some(ContentBlock::Image(
        ImageContent::new(data, mime).uri(Some(uri)),
    ))
}

/// An image as part of the agent's reply.
pub fn agent_image(block: ContentBlock) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(ContentChunk::new(block))
}

/// Send a completed tool call, with its image when it produced one.
pub async fn tool_completed_updates(
    call_id: &str,
    tool: &str,
    input: &Value,
    result: &ToolResult,
    workspace: &Path,
) -> Vec<SessionUpdate> {
    let image = tool_image(tool, input, result, workspace);
    if images_as_markdown() {
        // Tool output is shown as plain text there, so only `show_image`
        // pictures are sent, as Markdown in the reply.
        let mut updates = vec![tool_completed(call_id, "", result)];
        if let Some(ToolImage::Message(path)) = image
            && nca_core::tools::show_image::image_mime(&path).is_some()
            && tokio::fs::metadata(&path)
                .await
                .is_ok_and(|meta| meta.is_file())
        {
            updates.push(text_chunk(markdown_image(&path)));
        }
        return updates;
    }
    match image {
        Some(ToolImage::ToolOutput(path)) => {
            let image = image_block(&path).await;
            vec![tool_completed_with_image(call_id, "", result, image)]
        }
        Some(ToolImage::Message(path)) => {
            let mut updates = vec![tool_completed(call_id, "", result)];
            updates.extend(image_block(&path).await.map(agent_image));
            updates
        }
        None => vec![tool_completed(call_id, "", result)],
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;

    fn ok(output: &str) -> ToolResult {
        ToolResult {
            call_id: "c".into(),
            success: true,
            output: output.into(),
            error: None,
        }
    }

    #[test]
    fn finds_the_image_a_tool_produced() {
        let ws = Path::new("/work");
        assert_eq!(
            tool_image(
                "show_image",
                &serde_json::json!({"path": "a.png"}),
                &ok("Showing"),
                ws
            ),
            Some(ToolImage::Message(PathBuf::from("/work/a.png")))
        );
        let shot = "Saved a screenshot of Calculator (window 3026, 674x408 points) to /data/recordings/shot-1-calculator.png. Frontmost app unchanged: Dia.";
        assert_eq!(
            tool_image(
                "screen_record",
                &serde_json::json!({"action": "screenshot"}),
                &ok(shot),
                ws
            ),
            Some(ToolImage::ToolOutput(PathBuf::from(
                "/data/recordings/shot-1-calculator.png"
            )))
        );
        assert_eq!(
            tool_image(
                "screen_record",
                &serde_json::json!({"action": "stop"}),
                &ok(shot),
                ws
            ),
            None
        );
        assert_eq!(
            tool_image("read_file", &serde_json::json!({}), &ok("x.png"), ws),
            None
        );
    }

    #[test]
    fn markdown_images_use_the_absolute_path() {
        assert_eq!(
            markdown_image(Path::new("/Users/me/My Shots/shot-1.png")),
            "\n\n![shot-1](/Users/me/My%20Shots/shot-1.png)\n\n"
        );
    }

    #[tokio::test]
    async fn image_blocks_carry_base64_data_and_mime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("write");
        let block = image_block(&path).await.expect("image");
        let value = serde_json::to_value(&block).expect("json");
        assert_eq!(value["type"], "image");
        assert_eq!(value["mimeType"], "image/png");
        assert_eq!(value["data"], "iVBORw0KGgo=");
        assert!(image_block(&dir.path().join("missing.png")).await.is_none());
    }
}
