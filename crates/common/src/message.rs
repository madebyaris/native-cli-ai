use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// Role in a conversation turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// Reference to an on-disk image under the workspace (session attachments).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ImageAttachment {
    /// e.g. `image/png`, `image/jpeg`
    pub media_type: String,
    /// Path relative to workspace root, POSIX-style (`/` separators) in JSON.
    pub path: String,
}

/// One block inside a multimodal user message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        /// Path relative to workspace root (same as [`ImageAttachment::path`]).
        path: String,
    },
}

/// Message body: plain string (legacy + assistant/tool text) or structured parts (user multimodal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    pub fn text(s: impl Into<String>) -> Self {
        MessageContent::Text(s.into())
    }

    pub fn is_empty(&self) -> bool {
        match self {
            MessageContent::Text(t) => t.is_empty(),
            MessageContent::Parts(p) => p.is_empty(),
        }
    }

    /// Approximate character weight for context heuristics (images count as fixed overhead).
    pub fn approx_chars(&self) -> usize {
        match self {
            MessageContent::Text(t) => t.len(),
            MessageContent::Parts(parts) => {
                let mut n = 0usize;
                for p in parts {
                    match p {
                        ContentPart::Text { text } => n += text.len(),
                        ContentPart::Image { path, .. } => {
                            n += path.len() + 512;
                        }
                    }
                }
                n
            }
        }
    }

    pub fn has_image_parts(&self) -> bool {
        matches!(self, MessageContent::Parts(parts) if parts.iter().any(|p| matches!(p, ContentPart::Image { .. })))
    }

    /// Replace matching image parts with text placeholders after the image has already been
    /// processed and the on-disk attachment can be deleted.
    pub fn strip_image_paths(&mut self, removed_paths: &HashSet<String>) -> bool {
        let MessageContent::Parts(parts) = self else {
            return false;
        };

        let mut changed = false;
        for part in parts.iter_mut() {
            if let ContentPart::Image { path, .. } = part
                && removed_paths.contains(path)
            {
                let label = path.rsplit('/').next().unwrap_or(path);
                *part = ContentPart::Text {
                    text: format!("[image processed and removed after send: {label}]"),
                };
                changed = true;
            }
        }

        if parts
            .iter()
            .all(|part| matches!(part, ContentPart::Text { .. }))
        {
            let collapsed = parts
                .iter()
                .filter_map(|part| {
                    if let ContentPart::Text { text } = part {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            *self = MessageContent::Text(collapsed);
        }

        changed
    }

    /// Short string for events / TUI (no base64).
    pub fn event_preview(&self) -> String {
        match self {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Parts(parts) => {
                let mut text = String::new();
                let mut images = 0usize;
                for p in parts {
                    match p {
                        ContentPart::Text { text: t } => {
                            if !text.is_empty() && !t.is_empty() {
                                text.push(' ');
                            }
                            text.push_str(t);
                        }
                        ContentPart::Image { .. } => images += 1,
                    }
                }
                if images > 0 {
                    if !text.is_empty() && !text.chars().last().is_some_and(char::is_whitespace) {
                        text.push(' ');
                    }
                    text.push_str(&format!("[{images} image(s)]"));
                }
                text
            }
        }
    }

    /// Preview for user-authored content. Expanded ` ```file:path ` blocks are compacted back to
    /// `@path` so the transcript stays readable while the model still receives full file contents.
    pub fn user_event_preview(&self) -> String {
        collapse_expanded_file_blocks(&self.event_preview())
            .trim()
            .to_string()
    }

    /// Plain text only; images become placeholders (for summary prompts).
    pub fn to_summary_text(&self) -> String {
        match self {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Parts(parts) => {
                let mut out = String::new();
                for p in parts {
                    match p {
                        ContentPart::Text { text } => {
                            if !out.is_empty() && !text.is_empty() {
                                out.push('\n');
                            }
                            out.push_str(text);
                        }
                        ContentPart::Image { path, .. } => {
                            if !out.is_empty() {
                                out.push('\n');
                            }
                            out.push_str(&format!("[image attachment: {path}]"));
                        }
                    }
                }
                out
            }
        }
    }
}

fn collapse_expanded_file_blocks(text: &str) -> String {
    const HEADER: &str = "```file:";
    let mut out = String::new();
    let mut rest = text;

    while let Some(start) = rest.find(HEADER) {
        let (before, after_header_marker) = rest.split_at(start);
        out.push_str(before);

        let after_header = &after_header_marker[HEADER.len()..];
        let Some(header_end) = after_header.find('\n') else {
            out.push_str(after_header_marker);
            return collapse_excess_blank_lines(&out);
        };

        let path = after_header[..header_end].trim();
        let after_body = &after_header[header_end + 1..];
        let Some(block_end) = after_body.find("\n```") else {
            out.push_str(after_header_marker);
            return collapse_excess_blank_lines(&out);
        };

        trim_trailing_inline_whitespace(&mut out);
        if !out.is_empty() {
            out.push(' ');
        }
        out.push('@');
        out.push_str(path);
        rest = after_body[block_end + "\n```".len()..].trim_start();
        if !rest.is_empty() {
            out.push(' ');
        }
    }

    out.push_str(rest);
    collapse_excess_blank_lines(&out)
}

fn trim_trailing_inline_whitespace(text: &mut String) {
    let trimmed_len = text.trim_end().len();
    text.truncate(trimmed_len);
}

fn collapse_excess_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newline_run = 0usize;

    for ch in text.chars() {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push(ch);
            }
        } else {
            newline_run = 0;
            out.push(ch);
        }
    }

    out
}

mod message_content_serde {
    use super::{ContentPart, MessageContent};
    use serde::{Deserialize, Serialize, Serializer};

    pub fn serialize<S>(content: &MessageContent, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match content {
            MessageContent::Text(s) => serializer.serialize_str(s),
            MessageContent::Parts(parts) => parts.serialize(serializer),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<MessageContent, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        use serde_json::Value;
        let v = Value::deserialize(deserializer)?;
        match v {
            Value::String(s) => Ok(MessageContent::Text(s)),
            Value::Array(arr) => {
                let parts: Vec<ContentPart> =
                    serde_json::from_value(Value::Array(arr)).map_err(D::Error::custom)?;
                Ok(MessageContent::Parts(parts))
            }
            _ => Err(D::Error::custom(
                "message content must be a string or an array of content parts",
            )),
        }
    }
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    #[serde(with = "message_content_serde")]
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<MessageToolCall>>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn user_with_parts(parts: Vec<ContentPart>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Parts(parts),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Text(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<MessageToolCall>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Text(content.into()),
            tool_call_id: None,
            tool_calls: Some(tool_calls),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: MessageContent::Text(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: MessageContent::Text(content.into()),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }

    pub fn event_preview(&self) -> String {
        if self.role == Role::User {
            self.content.user_event_preview()
        } else {
            self.content.event_preview()
        }
    }
}

/// Tool call payload embedded in assistant history for replay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_legacy_string_content_roundtrips() {
        let json = r#"{"role":"user","content":"hello"}"#;
        let m: Message = serde_json::from_str(json).expect("parse");
        assert!(matches!(m.content, MessageContent::Text(ref s) if s == "hello"));
        let out = serde_json::to_string(&m).expect("ser");
        assert!(out.contains("\"hello\""));
    }

    #[test]
    fn serde_parts_array_roundtrips() {
        let m = Message::user_with_parts(vec![
            ContentPart::Text { text: "hi".into() },
            ContentPart::Image {
                media_type: "image/png".into(),
                path: ".nca/sessions/x/a.png".into(),
            },
        ]);
        let json = serde_json::to_string(&m).expect("ser");
        let m2: Message = serde_json::from_str(&json).expect("de");
        assert_eq!(m, m2);
    }

    #[test]
    fn strip_image_paths_replaces_images_with_text_placeholder() {
        let mut content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "look".into(),
            },
            ContentPart::Image {
                media_type: "image/png".into(),
                path: ".nca/sessions/x/a.png".into(),
            },
        ]);
        let removed = HashSet::from([".nca/sessions/x/a.png".to_string()]);

        assert!(content.strip_image_paths(&removed));
        assert!(
            matches!(content, MessageContent::Text(text) if text.contains("image processed and removed after send"))
        );
    }

    #[test]
    fn user_event_preview_compacts_expanded_file_blocks() {
        let msg = Message::user(
            "\n\n```file:.gitignore\n# content here\n```\n\n\nlearn about this project",
        );

        assert_eq!(msg.event_preview(), "@.gitignore learn about this project");
    }

    #[test]
    fn user_parts_preview_keeps_images_and_compacts_files() {
        let msg = Message::user_with_parts(vec![
            ContentPart::Text {
                text: "See\n\n```file:README.md\nhello\n```\n\n".into(),
            },
            ContentPart::Image {
                media_type: "image/png".into(),
                path: ".nca/sessions/x/a.png".into(),
            },
        ]);

        assert_eq!(msg.event_preview(), "See @README.md [1 image(s)]");
    }

    #[test]
    fn user_event_preview_keeps_multiple_mentions_inline() {
        let msg = Message::user(
            "compare ```file:Cargo.toml\n[package]\n```\n and ```file:README.md\nhello\n``` please",
        );

        assert_eq!(
            msg.event_preview(),
            "compare @Cargo.toml and @README.md please"
        );
    }
}
