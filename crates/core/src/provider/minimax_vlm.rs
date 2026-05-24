//! MiniMax Coding Plan vision API (`/v1/coding_plan/vlm`).
//!
//! The Anthropic-compatible `/v1/messages` path does not match how MiniMax exposes vision in
//! [MiniMax-Coding-Plan-MCP](https://github.com/MiniMax-AI/MiniMax-Coding-Plan-MCP): that stack
//! calls `POST /v1/coding_plan/vlm` with `prompt` + `image_url` (base64 data URL). We invoke the
//! same HTTP API from Rust so pasted images work without running the Python MCP.

use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use nca_common::message::{ContentPart, Message, MessageContent, Role};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use super::ProviderError;

/// Strip `/anthropic` from configured chat base URL to get the API origin for `/v1/coding_plan/*`.
pub fn minimax_api_origin(base_url: &str) -> String {
    let t = base_url.trim_end_matches('/');
    if let Some(prefix) = t.strip_suffix("/anthropic") {
        prefix.to_string()
    } else {
        t.to_string()
    }
}

fn workspace_file_path(workspace_root: &Path, rel: &str) -> std::path::PathBuf {
    let rel = rel.replace('\\', "/");
    workspace_root.join(rel)
}

/// Build `data:image/png;base64,...` from `media_type` like `image/png`.
fn data_url_for_image(media_type: &str, bytes: &[u8]) -> String {
    let mt = if media_type.starts_with("image/") {
        media_type.to_string()
    } else {
        format!("image/{media_type}")
    };
    let b64 = B64.encode(bytes);
    format!("data:{mt};base64,{b64}")
}

/// Call `POST {origin}/v1/coding_plan/vlm` (same contract as MiniMax Coding Plan MCP).
pub async fn coding_plan_vlm(
    client: &reqwest::Client,
    api_origin: &str,
    api_key: &str,
    prompt: &str,
    image_data_url: &str,
) -> Result<String, ProviderError> {
    let url = format!("{}/v1/coding_plan/vlm", api_origin.trim_end_matches('/'));

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|e| ProviderError::Configuration(format!("minimax vlm auth header: {e}")))?,
    );
    headers.insert("MM-API-Source", HeaderValue::from_static("nca"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let body = json!({
        "prompt": prompt,
        "image_url": image_data_url,
    });

    let response = client
        .post(&url)
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed(format!("coding_plan/vlm request: {e}")))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProviderError::RequestFailed(format!(
            "coding_plan/vlm HTTP {status}: {text}"
        )));
    }

    let v: Value = serde_json::from_str(&text).map_err(|e| {
        ProviderError::RequestFailed(format!("coding_plan/vlm invalid JSON: {e}; body: {text}"))
    })?;

    if let Some(br) = v.get("base_resp") {
        let code = br["status_code"].as_i64().unwrap_or(-1);
        if code != 0 {
            let msg = br["status_msg"].as_str().unwrap_or("error");
            return Err(ProviderError::RequestFailed(format!(
                "coding_plan/vlm API status {code}: {msg}"
            )));
        }
    }

    v.get("content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ProviderError::RequestFailed(format!(
                "coding_plan/vlm: empty or missing content in response: {}",
                text.chars().take(500).collect::<String>()
            ))
        })
}

/// For MiniMax: replace user `Parts` that contain images with a **text-only** user message whose
/// body includes VLM output from `/v1/coding_plan/vlm`, so `/v1/messages` receives usable context.
pub async fn materialize_minimax_user_images(
    messages: &[Message],
    workspace_root: &Path,
    client: &reqwest::Client,
    api_origin: &str,
    api_key: &str,
) -> Result<Vec<Message>, ProviderError> {
    let mut out = Vec::with_capacity(messages.len());

    for msg in messages {
        if msg.role != Role::User {
            out.push(msg.clone());
            continue;
        }

        let MessageContent::Parts(parts) = &msg.content else {
            out.push(msg.clone());
            continue;
        };

        if !parts.iter().any(|p| matches!(p, ContentPart::Image { .. })) {
            out.push(msg.clone());
            continue;
        }

        let mut sections: Vec<String> = Vec::new();
        let mut img_index = 0usize;

        let base_prompt = parts
            .iter()
            .filter_map(|p| {
                if let ContentPart::Text { text } = p {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();

        for p in parts {
            match p {
                ContentPart::Text { text } => {
                    let t = text.trim();
                    if !t.is_empty() {
                        sections.push(t.to_string());
                    }
                }
                ContentPart::Image { media_type, path } => {
                    img_index += 1;
                    let full = workspace_file_path(workspace_root, path);
                    let bytes = std::fs::read(&full).map_err(|e| {
                        ProviderError::RequestFailed(format!(
                            "read image for VLM {}: {e}",
                            full.display()
                        ))
                    })?;
                    let data_url = data_url_for_image(media_type, &bytes);

                    let prompt = if base_prompt.is_empty() {
                        "Describe this image in detail. Focus on any text, UI, diagrams, and errors relevant to software development."
                            .to_string()
                    } else {
                        format!(
                            "{base_prompt}\n\n(Image {img_index}: analyze the attached screenshot or image in the context of the question above.)"
                        )
                    };

                    let analysis =
                        coding_plan_vlm(client, api_origin, api_key, &prompt, &data_url).await?;

                    sections.push(format!(
                        "### Attached image {img_index} (MiniMax vision /v1/coding_plan/vlm)\n{analysis}"
                    ));
                }
            }
        }

        let mut combined = sections.join("\n\n");
        if combined.trim().is_empty() {
            combined = "(User sent image attachment(s) with no text.)".into();
        }

        out.push(Message {
            role: Role::User,
            content: MessageContent::Text(combined),
            tool_call_id: msg.tool_call_id.clone(),
            tool_calls: msg.tool_calls.clone(),
            reasoning_content: None,
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_strips_anthropic_suffix() {
        assert_eq!(
            minimax_api_origin("https://api.minimax.io/anthropic"),
            "https://api.minimax.io"
        );
        assert_eq!(
            minimax_api_origin("https://api.minimaxi.com/anthropic"),
            "https://api.minimaxi.com"
        );
    }

    #[test]
    fn data_url_format() {
        let u = data_url_for_image("image/png", &[1, 2, 3]);
        assert!(u.starts_with("data:image/png;base64,"));
    }
}
