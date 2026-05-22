//! Session transcript export (Markdown, JSON, HTML).

use crate::cli::{ExportArgs, ExportFormat};
use nca_common::config::NcaConfig;
use nca_common::message::{ContentPart, Message, MessageContent, Role};
use nca_common::session::SessionState;
use std::path::{Path, PathBuf};

pub(crate) async fn resolve_session_query(
    store: &nca_runtime::session_store::SessionStore,
    query: &str,
) -> anyhow::Result<String> {
    if query.is_empty() {
        anyhow::bail!("session id or prefix must not be empty");
    }
    let ids = store.list().await.map_err(anyhow::Error::msg)?;
    if ids.iter().any(|id| id == query) {
        return Ok(query.to_string());
    }
    let mut matches: Vec<String> = ids.into_iter().filter(|id| id.starts_with(query)).collect();
    matches.sort();
    match matches.as_slice() {
        [] => anyhow::bail!("no session id matches prefix {:?}", query),
        [one] => Ok(one.clone()),
        many => anyhow::bail!(
            "ambiguous session prefix {:?}: {} candidates (e.g. {:?}, {:?})",
            query,
            many.len(),
            many.first().map(String::as_str),
            many.get(1).map(String::as_str),
        ),
    }
}

pub async fn run_export(
    config: &NcaConfig,
    workspace_root: &Path,
    session_query: &str,
    args: ExportArgs,
) -> anyhow::Result<()> {
    let store = nca_runtime::session_store::SessionStore::new(
        workspace_root.join(&config.session.history_dir),
    );
    let session_id = resolve_session_query(&store, session_query).await?;
    let state = store.load(&session_id).await.map_err(anyhow::Error::msg)?;
    let workspace = state.meta.workspace.clone();

    let body = match args.format {
        ExportFormat::Json => {
            export_session_json(&state, args.include_system, args.include_tool_results)?
        }
        ExportFormat::Markdown => export_session_markdown(
            &state,
            &workspace,
            args.include_system,
            args.include_tool_results,
            args.inline_images,
        )?,
        ExportFormat::Html => export_session_html(
            &state,
            &workspace,
            args.include_system,
            args.include_tool_results,
            args.inline_images,
        )?,
    };

    if let Some(path) = args.output.as_deref() {
        tokio::fs::write(path, &body)
            .await
            .map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))?;
    } else {
        print!("{body}");
    }
    Ok(())
}

pub fn message_included(m: &Message, include_system: bool, include_tool_results: bool) -> bool {
    match m.role {
        Role::System => include_system,
        Role::Tool => include_tool_results,
        Role::User | Role::Assistant => true,
    }
}

pub fn export_session_json(
    state: &SessionState,
    include_system: bool,
    include_tool_results: bool,
) -> anyhow::Result<String> {
    let messages: Vec<&Message> = state
        .messages
        .iter()
        .filter(|m| message_included(m, include_system, include_tool_results))
        .collect();
    let payload = serde_json::json!({
        "id": state.meta.id,
        "created_at": state.meta.created_at,
        "updated_at": state.meta.updated_at,
        "workspace": state.meta.workspace,
        "model": state.meta.model,
        "status": state.meta.status,
        "parent_session_id": state.meta.parent_session_id,
        "child_session_ids": state.meta.child_session_ids,
        "total_input_tokens": state.total_input_tokens,
        "total_output_tokens": state.total_output_tokens,
        "estimated_cost_usd": state.estimated_cost_usd,
        "messages": messages,
    });
    Ok(serde_json::to_string_pretty(&payload)?)
}

pub fn export_session_markdown(
    state: &SessionState,
    workspace: &Path,
    include_system: bool,
    include_tool_results: bool,
    inline_images: bool,
) -> anyhow::Result<String> {
    let mut out = String::new();
    out.push_str(&format!("# Session `{}`\n\n", state.meta.id));
    out.push_str(&format!("- **Workspace:** `{}`\n", workspace.display()));
    out.push_str(&format!("- **Model:** {}\n", state.meta.model));
    out.push_str(&format!("- **Status:** {:?}\n", state.meta.status));
    out.push_str(&format!(
        "- **Updated:** {}\n",
        state.meta.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str(&format!(
        "- **Tokens:** in {} / out {} (~${:.4})\n\n",
        state.total_input_tokens, state.total_output_tokens, state.estimated_cost_usd
    ));

    for (i, msg) in state.messages.iter().enumerate() {
        if !message_included(msg, include_system, include_tool_results) {
            continue;
        }
        let title = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
            Role::Tool => "Tool",
        };
        out.push_str(&format!("## {title} (#{i})\n\n"));
        if let Some(id) = &msg.tool_call_id {
            out.push_str(&format!("`tool_call_id`: `{id}`\n\n"));
        }
        if let Some(calls) = &msg.tool_calls {
            out.push_str("**Tool calls:**\n\n");
            for c in calls {
                let args = serde_json::to_string_pretty(&c.arguments).unwrap_or_default();
                out.push_str(&format!(
                    "- `{}` (`{}`)\n\n```json\n{}\n```\n\n",
                    c.name, c.id, args
                ));
            }
        }
        out.push_str(&markdown_for_content(
            &msg.content,
            workspace,
            inline_images,
        )?);
        out.push_str("\n\n---\n\n");
    }
    Ok(out)
}

pub fn export_session_html(
    state: &SessionState,
    workspace: &Path,
    include_system: bool,
    include_tool_results: bool,
    inline_images: bool,
) -> anyhow::Result<String> {
    let mut blocks = String::new();
    for (i, msg) in state.messages.iter().enumerate() {
        if !message_included(msg, include_system, include_tool_results) {
            continue;
        }
        let title = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
            Role::Tool => "Tool",
        };
        blocks.push_str("<section class=\"msg\">");
        blocks.push_str(&format!(
            "<h2>{} <span class=\"idx\">#{}</span></h2>",
            html_escape(title),
            i
        ));
        if let Some(id) = &msg.tool_call_id {
            blocks.push_str(&format!(
                "<p class=\"meta\"><code>tool_call_id</code>: {}</p>",
                html_escape(id)
            ));
        }
        if let Some(calls) = &msg.tool_calls {
            blocks.push_str("<div class=\"tool-calls\"><h3>Tool calls</h3><ul>");
            for c in calls {
                let args = serde_json::to_string_pretty(&c.arguments).unwrap_or_default();
                blocks.push_str("<li><code>");
                blocks.push_str(&html_escape(&c.name));
                blocks.push_str("</code> <span class=\"id\">");
                blocks.push_str(&html_escape(&c.id));
                blocks.push_str("</span><pre>");
                blocks.push_str(&html_escape(&args));
                blocks.push_str("</pre></li>");
            }
            blocks.push_str("</ul></div>");
        }
        blocks.push_str(&html_for_content(&msg.content, workspace, inline_images)?);
        blocks.push_str("</section>");
    }

    let doc = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8"/>
  <title>nca session {}</title>
  <style>
    body {{ font-family: system-ui, sans-serif; max-width: 52rem; margin: 2rem auto; line-height: 1.5; }}
    h1 {{ font-size: 1.25rem; }}
    section.msg {{ border-bottom: 1px solid #ccc; padding: 1rem 0; }}
    pre {{ background: #f6f8fa; padding: 0.75rem; overflow-x: auto; }}
    img {{ max-width: 100%; height: auto; }}
    .idx {{ color: #666; font-weight: normal; font-size: 0.9em; }}
  </style>
</head>
<body>
  <h1>Session <code>{}</code></h1>
  <p>{} · {} · in {} / out {} tokens (~${:.4})</p>
  {}
</body>
</html>
"#,
        html_escape(&state.meta.id),
        html_escape(&state.meta.id),
        html_escape(&state.meta.model),
        html_escape(&format!("{:?}", state.meta.status)),
        state.total_input_tokens,
        state.total_output_tokens,
        state.estimated_cost_usd,
        blocks
    );
    Ok(doc)
}

pub fn markdown_for_content(
    content: &MessageContent,
    workspace: &Path,
    inline_images: bool,
) -> anyhow::Result<String> {
    match content {
        MessageContent::Text(t) => Ok(t.clone()),
        MessageContent::Parts(parts) => {
            let mut s = String::new();
            for p in parts {
                match p {
                    ContentPart::Text { text } => {
                        s.push_str(text);
                        s.push('\n');
                    }
                    ContentPart::Image { media_type, path } => {
                        let src = image_src(path, media_type, workspace, inline_images)?;
                        let label = path.rsplit('/').next().unwrap_or(path);
                        s.push_str(&format!("![{label}]({src})\n\n"));
                    }
                }
            }
            Ok(s)
        }
    }
}

pub fn html_for_content(
    content: &MessageContent,
    workspace: &Path,
    inline_images: bool,
) -> anyhow::Result<String> {
    match content {
        MessageContent::Text(t) => Ok(format!("<div class=\"text\">{}</div>", text_to_html(t))),
        MessageContent::Parts(parts) => {
            let mut s = String::new();
            s.push_str("<div class=\"parts\">");
            for p in parts {
                match p {
                    ContentPart::Text { text } => {
                        s.push_str(&format!("<div class=\"text\">{}</div>", text_to_html(text)));
                    }
                    ContentPart::Image { media_type, path } => {
                        let src = image_src(path, media_type, workspace, inline_images)?;
                        let alt = html_escape(path);
                        s.push_str(&format!(
                            r#"<figure class="img"><img src="{}" alt="{}"/></figure>"#,
                            html_escape(&src),
                            alt
                        ));
                    }
                }
            }
            s.push_str("</div>");
            Ok(s)
        }
    }
}

pub fn image_src(
    path: &str,
    media_type: &str,
    workspace: &Path,
    inline: bool,
) -> anyhow::Result<String> {
    if !inline {
        return Ok(path.to_string());
    }
    let full = workspace.join(path);
    let bytes =
        std::fs::read(&full).map_err(|e| anyhow::anyhow!("read image {}: {e}", full.display()))?;
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    Ok(format!("data:{media_type};base64,{b64}"))
}

pub fn text_to_html(s: &str) -> String {
    let escaped = html_escape(s);
    escaped.replace('\n', "<br/>\n")
}

pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}
