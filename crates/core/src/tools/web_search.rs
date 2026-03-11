use nca_common::config::WebConfig;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use scraper::{Html, Selector};
use std::time::Duration;

use super::ToolExecutor;

pub struct WebSearchTool {
    client: reqwest::Client,
    config: WebConfig,
}

impl WebSearchTool {
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
impl ToolExecutor for WebSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_search".into(),
            description: "Search the public web and return titles, URLs, and snippets".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let query = call.input["query"].as_str().unwrap_or("").trim();
        let limit = call.input["limit"]
            .as_u64()
            .map(|v| v as usize)
            .unwrap_or(self.config.default_search_limit)
            .clamp(1, 10);

        if query.is_empty() {
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some("query is required".into()),
            };
        }

        let response = self
            .client
            .get("https://html.duckduckgo.com/html/")
            .query(&[("q", query)])
            .send()
            .await;

        let body = match response {
            Ok(response) => match response.text().await {
                Ok(body) => body,
                Err(err) => {
                    return ToolResult {
                        call_id: call.id.clone(),
                        success: false,
                        output: String::new(),
                        error: Some(format!("failed to read search response: {err}")),
                    };
                }
            },
            Err(err) => {
                return ToolResult {
                    call_id: call.id.clone(),
                    success: false,
                    output: String::new(),
                    error: Some(format!("search request failed: {err}")),
                };
            }
        };

        let document = Html::parse_document(&body);
        let result_selector = Selector::parse(".result").unwrap();
        let title_selector = Selector::parse(".result__title a, a.result__a").unwrap();
        let snippet_selector = Selector::parse(".result__snippet").unwrap();

        let mut rows = Vec::new();
        for result in document.select(&result_selector).take(limit) {
            let title_node = result.select(&title_selector).next();
            let snippet = result
                .select(&snippet_selector)
                .next()
                .map(|node| clean_text(&node.text().collect::<Vec<_>>().join(" ")))
                .unwrap_or_default();

            if let Some(title_node) = title_node {
                let title = clean_text(&title_node.text().collect::<Vec<_>>().join(" "));
                let url = title_node
                    .value()
                    .attr("href")
                    .map(clean_href)
                    .unwrap_or_default();
                if !title.is_empty() && !url.is_empty() {
                    rows.push(format!("- {title}\n  URL: {url}\n  Snippet: {snippet}"));
                }
            }
        }

        if rows.is_empty() {
            let fallback = clean_text(&extract_text(&document));
            return ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: fallback.chars().take(self.config.max_fetch_chars).collect(),
                error: Some("no structured search results parsed".into()),
            };
        }

        ToolResult {
            call_id: call.id.clone(),
            success: true,
            output: rows.join("\n"),
            error: None,
        }
    }
}

fn clean_href(href: &str) -> String {
    if let Some(stripped) = href.strip_prefix("//") {
        format!("https://{stripped}")
    } else {
        href.to_string()
    }
}

fn extract_text(document: &Html) -> String {
    let body_selector = Selector::parse("body").unwrap();
    document
        .select(&body_selector)
        .next()
        .map(|node| node.text().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
