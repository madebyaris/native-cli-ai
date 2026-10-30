use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use std::path::{Path, PathBuf};

use super::ToolExecutor;

/// Largest image `show_image` will send. Clients receive it base64-encoded
/// inside one JSON-RPC line, so a bigger file would stall the stream.
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Shows an image file to the user. Front ends that can render images (ACP
/// clients such as Termul or Zed) display it in the chat; others print the path.
pub struct ShowImageTool {
    workspace_root: PathBuf,
}

impl ShowImageTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

/// MIME type for the image formats chat clients render, by file extension.
pub fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Resolve a user-facing path: `~/` is the home folder, a relative path is
/// taken from the workspace, an absolute path is used as-is.
pub fn resolve_image_path(raw: &str, workspace_root: &Path) -> PathBuf {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

#[async_trait::async_trait]
impl ToolExecutor for ShowImageTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "show_image".into(),
            description: "Show an image file (PNG, JPEG, GIF, WebP) to the user in the chat. \
                          Use it to display screenshots, charts, or generated images instead of \
                          only giving a path. Clients that cannot render images show the path."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Image path: absolute, `~/...`, or relative to the workspace"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let failed = |error: String| ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: String::new(),
            error: Some(error),
        };
        let Some(raw) = call.input["path"].as_str().filter(|p| !p.trim().is_empty()) else {
            return failed("`path` is required".into());
        };
        let path = resolve_image_path(raw, &self.workspace_root);
        let Some(mime) = image_mime(&path) else {
            return failed(format!(
                "{} is not a PNG, JPEG, GIF, or WebP image",
                path.display()
            ));
        };
        let meta = match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => return failed(format!("{} is not a file", path.display())),
            Err(error) => return failed(format!("cannot read {}: {error}", path.display())),
        };
        if meta.len() > MAX_IMAGE_BYTES {
            return failed(format!(
                "{} is {} KB; the limit is {} KB. Save a smaller copy (e.g. `sips -Z 1600 in.png --out small.png`) and show that",
                path.display(),
                meta.len() / 1024,
                MAX_IMAGE_BYTES / 1024
            ));
        }
        ToolResult {
            call_id: call.id.clone(),
            success: true,
            output: format!(
                "Showing image {} ({} KB, {mime}).",
                path.display(),
                meta.len() / 1024
            ),
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(path: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: "show_image".into(),
            input: serde_json::json!({ "path": path }),
        }
    }

    #[tokio::test]
    async fn shows_an_image_in_the_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("shot.png"), b"\x89PNG\r\n\x1a\n").expect("write");
        let tool = ShowImageTool::new(dir.path().to_path_buf());
        let result = tool.execute(&call("shot.png")).await;
        assert!(result.success, "{result:?}");
        assert!(result.output.contains("shot.png"));
        assert!(result.output.contains("image/png"));
    }

    #[tokio::test]
    async fn rejects_missing_files_and_non_images() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("notes.txt"), "hi").expect("write");
        let tool = ShowImageTool::new(dir.path().to_path_buf());
        assert!(!tool.execute(&call("notes.txt")).await.success);
        assert!(!tool.execute(&call("missing.png")).await.success);
        assert!(!tool.execute(&call("")).await.success);
    }

    #[test]
    fn resolves_home_relative_and_absolute_paths() {
        let root = Path::new("/work");
        assert_eq!(
            resolve_image_path("a.png", root),
            PathBuf::from("/work/a.png")
        );
        assert_eq!(
            resolve_image_path("/tmp/a.png", root),
            PathBuf::from("/tmp/a.png")
        );
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(
                resolve_image_path("~/Desktop/a.png", root),
                PathBuf::from(home).join("Desktop/a.png")
            );
        }
        assert_eq!(image_mime(Path::new("x.JPG")), Some("image/jpeg"));
        assert_eq!(image_mime(Path::new("x.mov")), None);
    }
}
