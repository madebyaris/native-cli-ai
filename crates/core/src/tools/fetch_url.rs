use nca_common::config::WebConfig;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use scraper::{Html, Selector};
use std::time::Duration;

use super::ToolExecutor;

pub struct FetchUrlTool {
    client: reqwest::Client,
    config: WebConfig,
}

impl FetchUrlTool {
    pub fn new(config: WebConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(config.user_agent.clone())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, config }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for FetchUrlTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "fetch_url".into(),
            description: "Fetch and normalize the text content of a URL".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let url = call.input["url"].as_str().unwrap_or("").trim();
        if url.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("url is required".into()),
            };
        }

        let response = self.client.get(url).send().await;
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("fetch failed: {err}")),
                };
            }
        };

        let status = response.status();
        if !status.is_success() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(format!("unexpected status: {status}")),
            };
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = match response.text().await {
            Ok(body) => body,
            Err(err) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("failed to read response body: {err}")),
                };
            }
        };

        let normalized = if content_type.contains("html") || body.contains("<html") {
            normalize_html(&body)
        } else {
            normalize_plain_text(&body)
        };

        ToolResult {
            call_id: call.id.clone(),
            success: true,
            output: normalized.chars().take(self.config.max_fetch_chars).collect(),
            error: None,
        }
    }
}

fn normalize_html(body: &str) -> String {
    let document = Html::parse_document(body);
    let title_selector = Selector::parse("title").unwrap();
    let body_selector = Selector::parse("body").unwrap();

    let title = document
        .select(&title_selector)
        .next()
        .map(|node| normalize_plain_text(&node.text().collect::<Vec<_>>().join(" ")))
        .filter(|text| !text.is_empty());
    let content = document
        .select(&body_selector)
        .next()
        .map(|node| normalize_plain_text(&node.text().collect::<Vec<_>>().join(" ")))
        .unwrap_or_default();

    match title {
        Some(title) if !content.is_empty() => format!("Title: {title}\n\n{content}"),
        Some(title) => title,
        None => content,
    }
}

fn normalize_plain_text(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}
