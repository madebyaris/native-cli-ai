//! Keyless web search.
//!
//! There is no dependable search API without a key, so this scrapes the HTML of engines
//! that tolerate plain HTTP clients. No single engine is dependable: on some networks
//! DuckDuckGo refuses connections, Bing answers with results for a different query, and
//! Mojeek returns a bot challenge. The tool therefore walks a configurable list of
//! backends and uses the first one that returns results matching the query.

use nca_common::config::WebConfig;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use scraper::{ElementRef, Html, Selector};
use std::fmt::Write as _;
use std::time::Duration;

use super::ToolExecutor;

/// Search engines answer a client that identifies itself as a bot with a challenge page
/// or unrelated results, so search requests present as a browser. `web.search_user_agent`
/// overrides it.
const DEFAULT_SEARCH_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Shortest query token that can prove a result set is on topic. Two-character tokens
/// ("ai", "go") match too much to be evidence either way.
const MIN_RELEVANCE_TOKEN_CHARS: usize = 3;

pub struct WebSearchTool {
    client: reqwest::Client,
    config: WebConfig,
}

impl WebSearchTool {
    pub fn new(config: WebConfig) -> Self {
        let user_agent = config
            .search_user_agent
            .clone()
            .unwrap_or_else(|| DEFAULT_SEARCH_USER_AGENT.to_string());
        // One attempt per backend, each bounded by the configured timeout.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(user_agent)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, config }
    }

    /// Configured backends, de-duplicated. Unknown names are reported rather than silently
    /// skipped, because a typo would otherwise look like a network failure.
    fn plan(&self) -> (Vec<Backend>, Vec<String>) {
        let mut backends = Vec::new();
        let mut unknown = Vec::new();
        for name in &self.config.search_backends {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            match Backend::from_name(name) {
                Some(backend) if !backends.contains(&backend) => backends.push(backend),
                Some(_) => {}
                None => unknown.push(name.to_string()),
            }
        }
        (backends, unknown)
    }

    /// The API key for a keyed backend: the configured key, else its environment variable.
    fn api_key(&self, backend: Backend) -> Option<String> {
        if let Some(key) = self.config.search_keys.get(backend.name())
            && !key.trim().is_empty()
        {
            return Some(key.trim().to_string());
        }
        let value = std::env::var(backend.key_env()?).ok()?;
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    }

    async fn fetch(
        &self,
        backend: Backend,
        query: &str,
        limit: usize,
        key: Option<&str>,
    ) -> Result<Vec<SearchHit>, String> {
        match backend {
            Backend::Exa => {
                let body = serde_json::json!({
                    "query": query,
                    "numResults": limit,
                    "type": "auto",
                    "contents": { "highlights": true },
                });
                let value = self
                    .post_json(
                        backend.endpoint(),
                        body,
                        &[("x-api-key", key.unwrap_or_default())],
                    )
                    .await?;
                Ok(parse_exa(&value))
            }
            Backend::Tavily => {
                let body = serde_json::json!({
                    "query": query,
                    "max_results": limit,
                    "search_depth": "basic",
                });
                let bearer = format!("Bearer {}", key.unwrap_or_default());
                let value = self
                    .post_json(backend.endpoint(), body, &[("Authorization", &bearer)])
                    .await?;
                Ok(parse_tavily(&value))
            }
            Backend::Serper => {
                let body = serde_json::json!({ "q": query, "num": limit });
                let value = self
                    .post_json(
                        backend.endpoint(),
                        body,
                        &[("X-API-KEY", key.unwrap_or_default())],
                    )
                    .await?;
                Ok(parse_serper(&value))
            }
            Backend::BraveApi => {
                let count = limit.to_string();
                let response = self
                    .client
                    .get(backend.endpoint())
                    .query(&[("q", query), ("count", count.as_str())])
                    .header(reqwest::header::ACCEPT, "application/json")
                    .header("X-Subscription-Token", key.unwrap_or_default())
                    .send()
                    .await
                    .map_err(|err| format!("request failed: {err}"))?;
                let value = self.json_body(response).await?;
                Ok(parse_brave_api(&value))
            }
            _ => {
                let response = self
                    .client
                    .get(backend.endpoint())
                    .query(&[("q", query)])
                    .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
                    .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
                    .send()
                    .await
                    .map_err(|err| format!("request failed: {err}"))?;

                let status = response.status();
                if !status.is_success() {
                    return Err(format!("HTTP {status}"));
                }
                let body = response
                    .text()
                    .await
                    .map_err(|err| format!("failed to read response: {err}"))?;
                Ok(backend.parse(&body))
            }
        }
    }

    async fn post_json(
        &self,
        endpoint: &str,
        body: serde_json::Value,
        headers: &[(&str, &str)],
    ) -> Result<serde_json::Value, String> {
        let mut request = self.client.post(endpoint).json(&body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("request failed: {err}"))?;
        self.json_body(response).await
    }

    /// Reads a JSON body. Search APIs report a bad key as 401/403/422 with a JSON
    /// explanation, so the status and that explanation are both kept.
    async fn json_body(&self, response: reqwest::Response) -> Result<serde_json::Value, String> {
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| format!("failed to read response: {err}"))?;
        if !status.is_success() {
            let detail = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| error_message(&value))
                .unwrap_or_else(|| clean_text(&body).chars().take(200).collect());
            return Err(format!("HTTP {status}: {detail}"));
        }
        serde_json::from_str(&body).map_err(|err| format!("invalid JSON response: {err}"))
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
        let query = call.input["query"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
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

        let (backends, unknown) = self.plan();
        let mut failures: Vec<String> = unknown
            .into_iter()
            .map(|name| format!("{name}: unknown backend"))
            .collect();
        let mut skipped: Vec<String> = Vec::new();

        for backend in backends {
            let key = self.api_key(backend);
            if backend.key_env().is_some() && key.is_none() {
                skipped.push(format!(
                    "{}: no API key (set web.search_keys.{} or {})",
                    backend.name(),
                    backend.name(),
                    backend.key_env().unwrap_or_default()
                ));
                continue;
            }
            match self.fetch(backend, &query, limit, key.as_deref()).await {
                Ok(hits) if hits.is_empty() => {
                    failures.push(format!("{}: no results parsed", backend.name()));
                }
                Ok(hits) if !matches_query(&hits, &query) => {
                    failures.push(format!(
                        "{}: results did not match the query",
                        backend.name()
                    ));
                }
                Ok(hits) => {
                    return ToolResult {
                        call_id: call.id.clone(),
                        success: true,
                        output: render(&hits, limit),
                        error: None,
                    };
                }
                Err(err) => failures.push(format!("{}: {err}", backend.name())),
            }
        }

        let mut details = failures;
        details.extend(skipped);
        ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: String::new(),
            error: Some(format!(
                "every search backend failed ({}) — set `web.search_backends` to choose different ones",
                details.join("; ")
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchHit {
    title: String,
    url: String,
    snippet: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Exa,
    Tavily,
    Serper,
    BraveApi,
    Brave,
    DuckDuckGo,
    Bing,
    Mojeek,
}

impl Backend {
    fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "exa" => Some(Self::Exa),
            "tavily" => Some(Self::Tavily),
            "serper" => Some(Self::Serper),
            "brave-api" | "brave_api" => Some(Self::BraveApi),
            "brave" => Some(Self::Brave),
            "duckduckgo" | "ddg" => Some(Self::DuckDuckGo),
            "bing" => Some(Self::Bing),
            "mojeek" => Some(Self::Mojeek),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Exa => "exa",
            Self::Tavily => "tavily",
            Self::Serper => "serper",
            Self::BraveApi => "brave-api",
            Self::Brave => "brave",
            Self::DuckDuckGo => "duckduckgo",
            Self::Bing => "bing",
            Self::Mojeek => "mojeek",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::Exa => "https://api.exa.ai/search",
            Self::Tavily => "https://api.tavily.com/search",
            Self::Serper => "https://google.serper.dev/search",
            Self::BraveApi => "https://api.search.brave.com/res/v1/web/search",
            Self::Brave => "https://search.brave.com/search",
            Self::DuckDuckGo => "https://html.duckduckgo.com/html/",
            Self::Bing => "https://www.bing.com/search",
            Self::Mojeek => "https://www.mojeek.com/search",
        }
    }

    /// Environment variable consulted when `web.search_keys` has no entry.
    /// `None` marks a keyless scraper.
    fn key_env(self) -> Option<&'static str> {
        match self {
            Self::Exa => Some("EXA_API_KEY"),
            Self::Tavily => Some("TAVILY_API_KEY"),
            Self::Serper => Some("SERPER_API_KEY"),
            Self::BraveApi => Some("BRAVE_API_KEY"),
            _ => None,
        }
    }

    fn parse(self, body: &str) -> Vec<SearchHit> {
        match self {
            Self::Brave => parse_selector(body, brave_selectors()),
            Self::DuckDuckGo => parse_selector(body, duckduckgo_selectors()),
            Self::Bing => parse_selector(body, bing_selectors()),
            Self::Mojeek => parse_selector(body, mojeek_selectors()),
            _ => Vec::new(),
        }
    }
}

/// Best human-readable explanation from a search API's error body.
fn error_message(value: &serde_json::Value) -> Option<String> {
    for pointer in [
        "/error/message",
        "/error/detail",
        "/error",
        "/detail/error",
        "/detail",
        "/message",
    ] {
        if let Some(text) = value.pointer(pointer).and_then(|node| node.as_str())
            && !text.trim().is_empty()
        {
            return Some(clean_text(text));
        }
    }
    None
}

fn json_hits(value: &serde_json::Value, path: &[&str], fields: HitFields) -> Vec<SearchHit> {
    let mut node = value;
    for key in path {
        node = &node[*key];
    }
    let Some(items) = node.as_array() else {
        return Vec::new();
    };

    let mut hits = Vec::new();
    for item in items {
        let title = json_string(item, fields.title);
        let url = json_string(item, fields.url);
        if title.is_empty() || url.is_empty() {
            continue;
        }
        hits.push(SearchHit {
            title,
            url,
            snippet: fields
                .snippet
                .iter()
                .find_map(|key| non_empty(json_string(item, &[*key])))
                .unwrap_or_default(),
        });
    }
    hits
}

/// Which keys in a provider's result object hold the three fields.
struct HitFields {
    title: &'static [&'static str],
    url: &'static [&'static str],
    snippet: &'static [&'static str],
}

fn json_string(item: &serde_json::Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| item.get(*key).and_then(|node| node.as_str()))
        .map_or_else(String::new, clean_text)
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

/// Exa: `results[]` with `title`, `url`, and highlights or extracted text.
fn parse_exa(value: &serde_json::Value) -> Vec<SearchHit> {
    let mut hits = json_hits(
        value,
        &["results"],
        HitFields {
            title: &["title"],
            url: &["url"],
            snippet: &["summary", "highlights", "text"],
        },
    );
    // `highlights` is an array; fold the first entry into the snippet.
    if let Some(items) = value["results"].as_array() {
        for (hit, item) in hits.iter_mut().zip(items) {
            if hit.snippet.is_empty()
                && let Some(first) = item["highlights"].as_array().and_then(|list| list.first())
                && let Some(text) = first.as_str()
            {
                hit.snippet = clean_text(text);
            }
            if hit.snippet.chars().count() > 400 {
                hit.snippet = hit.snippet.chars().take(400).collect();
            }
        }
    }
    hits
}

/// Tavily: `results[]` with `title`, `url`, `content`.
fn parse_tavily(value: &serde_json::Value) -> Vec<SearchHit> {
    json_hits(
        value,
        &["results"],
        HitFields {
            title: &["title"],
            url: &["url"],
            snippet: &["content", "raw_content"],
        },
    )
}

/// Serper: `organic[]` with `title`, `link`, `snippet`.
fn parse_serper(value: &serde_json::Value) -> Vec<SearchHit> {
    json_hits(
        value,
        &["organic"],
        HitFields {
            title: &["title"],
            url: &["link", "url"],
            snippet: &["snippet", "description"],
        },
    )
}

/// Brave Search API: `web.results[]` with `title`, `url`, `description`.
fn parse_brave_api(value: &serde_json::Value) -> Vec<SearchHit> {
    json_hits(
        value,
        &["web", "results"],
        HitFields {
            title: &["title"],
            url: &["url"],
            snippet: &["description"],
        },
    )
}

/// Selectors are parsed once per process rather than once per result.
struct SelectorSet {
    result: Selector,
    title: Selector,
    /// Shown URL, used when the title is not itself a link.
    url: Selector,
    snippet: Selector,
    /// Link whose `href` is the destination.
    link: Selector,
}

fn selector(source: &str) -> Selector {
    Selector::parse(source).unwrap_or_else(|err| panic!("invalid selector {source}: {err:?}"))
}

// Brave renders results server-side. Its classes carry Svelte hashes, so match the
// semantic class names and the stable data attribute.
fn brave_selectors() -> &'static SelectorSet {
    static SET: std::sync::OnceLock<SelectorSet> = std::sync::OnceLock::new();
    SET.get_or_init(|| SelectorSet {
        result: selector(r#"div[data-type="web"]"#),
        title: selector(".snippet-title"),
        url: selector("cite.snippet-url"),
        snippet: selector(".generic-snippet .content, .snippet-description"),
        link: selector("a[href]"),
    })
}

fn duckduckgo_selectors() -> &'static SelectorSet {
    static SET: std::sync::OnceLock<SelectorSet> = std::sync::OnceLock::new();
    SET.get_or_init(|| SelectorSet {
        result: selector(".result"),
        title: selector(".result__title, a.result__a"),
        url: selector(".result__url"),
        snippet: selector(".result__snippet"),
        link: selector("a.result__a"),
    })
}

fn bing_selectors() -> &'static SelectorSet {
    static SET: std::sync::OnceLock<SelectorSet> = std::sync::OnceLock::new();
    SET.get_or_init(|| SelectorSet {
        result: selector("li.b_algo"),
        title: selector("h2"),
        url: selector("cite"),
        snippet: selector(".b_caption"),
        link: selector("h2 a"),
    })
}

fn mojeek_selectors() -> &'static SelectorSet {
    static SET: std::sync::OnceLock<SelectorSet> = std::sync::OnceLock::new();
    SET.get_or_init(|| SelectorSet {
        result: selector("ul.results-standard li"),
        title: selector("h2 a, a.title"),
        url: selector(".url"),
        snippet: selector(".s"),
        link: selector("h2 a, a.title"),
    })
}

fn parse_selector(body: &str, set: &SelectorSet) -> Vec<SearchHit> {
    let document = Html::parse_document(body);
    let mut hits = Vec::new();
    for result in document.select(&set.result) {
        let title = text_of(result.select(&set.title).next());
        let url = match result
            .select(&set.link)
            .next()
            .and_then(|node| node.value().attr("href"))
        {
            Some(href) => clean_href(href),
            None => display_url(text_of(result.select(&set.url).next())),
        };
        let snippet = text_of(result.select(&set.snippet).next());

        if title.is_empty() || url.is_empty() {
            continue;
        }
        hits.push(SearchHit {
            title,
            url,
            snippet,
        });
    }
    hits
}

fn text_of(node: Option<ElementRef<'_>>) -> String {
    node.map(|node| clean_text(&node.text().collect::<Vec<_>>().join(" ")))
        .unwrap_or_default()
}

fn clean_href(href: &str) -> String {
    let href = href.trim();
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    href.to_string()
}

/// Bing and Mojeek show a trimmed breadcrumb ("docs.rs › zerocopy") instead of a full URL.
fn display_url(text: String) -> String {
    if text.is_empty() {
        return text;
    }
    let path = text.replace(" › ", "/");
    if path.starts_with("http://") || path.starts_with("https://") {
        path
    } else {
        format!("https://{path}")
    }
}

/// Whether any meaningful query token appears in the results.
///
/// A decoy response is a successful HTTP reply whose results answer a different query, so
/// results are only accepted when the query is visible in them. A query with no token long
/// enough to be evidence is accepted as-is.
fn matches_query(hits: &[SearchHit], query: &str) -> bool {
    let tokens: Vec<String> = query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| token.chars().count() >= MIN_RELEVANCE_TOKEN_CHARS)
        .map(|token| token.to_lowercase())
        .collect();

    if tokens.is_empty() {
        return true;
    }

    let corpus = hits
        .iter()
        .map(|hit| format!("{} {} {}", hit.title, hit.url, hit.snippet))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    tokens.iter().any(|token| corpus.contains(token.as_str()))
}

fn render(hits: &[SearchHit], limit: usize) -> String {
    let mut output = String::new();
    for hit in hits.iter().take(limit) {
        let _ = write!(
            output,
            "- {}\n  URL: {}\n  Snippet: {}\n",
            hit.title, hit.url, hit.snippet
        );
    }
    output.trim_end().to_string()
}

fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(backends: &[&str]) -> WebSearchTool {
        let config = WebConfig {
            timeout_secs: 1,
            search_backends: backends.iter().map(|name| name.to_string()).collect(),
            ..WebConfig::default()
        };
        WebSearchTool::new(config)
    }

    fn call(query: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "web_search".into(),
            input: serde_json::json!({ "query": query, "limit": 5 }),
        }
    }

    #[test]
    fn backend_names_are_case_insensitive_and_aliased() {
        assert_eq!(Backend::from_name("Brave"), Some(Backend::Brave));
        assert_eq!(Backend::from_name("ddg"), Some(Backend::DuckDuckGo));
        assert_eq!(Backend::from_name("bing"), Some(Backend::Bing));
        assert_eq!(Backend::from_name("nope"), None);
    }

    #[test]
    fn plan_skips_unknown_and_duplicate_backends() {
        let tool = tool(&["brave", "nope", "brave", "bing"]);
        let (backends, unknown) = tool.plan();

        assert_eq!(backends, vec![Backend::Brave, Backend::Bing]);
        assert_eq!(unknown, vec!["nope".to_string()]);
    }

    #[test]
    fn brave_results_parse_with_link_title_and_snippet() {
        let body = r#"
            <div data-type="web"><div class="result-body"><div class="result-wrapper">
              <div class="result-content">
                <a href="https://docs.rs/zerocopy" class="l1"><div class="site-name-content">
                  <cite class="snippet-url">docs.rs</cite></div>
                  <div class="title snippet-title">zerocopy - Rust</div>
                </a>
                <div class="generic-snippet"><div class="content">Safe byte conversions.</div></div>
              </div>
            </div></div></div>
        "#;

        let hits = Backend::Brave.parse(body);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://docs.rs/zerocopy");
        assert!(hits[0].title.contains("zerocopy"));
        assert!(hits[0].snippet.contains("Safe byte conversions"));
    }

    #[test]
    fn duckduckgo_results_parse() {
        let body = r#"
            <div class="result">
              <div class="result__title"><a class="result__a" href="https://example.com/a">Example title</a></div>
              <div class="result__snippet">A snippet about the topic.</div>
            </div>
        "#;

        let hits = Backend::DuckDuckGo.parse(body);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Example title");
        assert_eq!(hits[0].url, "https://example.com/a");
        assert_eq!(hits[0].snippet, "A snippet about the topic.");
    }

    #[test]
    fn bing_results_parse_and_breadcrumbs_become_urls() {
        let body = r#"
            <li class="b_algo">
              <h2><a href="https://www.bing.com/ck/a?u=opaque">zerocopy crate</a></h2>
              <cite>docs.rs › zerocopy</cite>
              <div class="b_caption"><p>Byte conversion.</p></div>
            </li>
        "#;

        let hits = Backend::Bing.parse(body);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://www.bing.com/ck/a?u=opaque");
        assert_eq!(
            display_url("docs.rs › zerocopy".into()),
            "https://docs.rs/zerocopy"
        );
        assert_eq!(
            display_url("https://docs.rs › zerocopy".into()),
            "https://docs.rs/zerocopy"
        );
    }

    #[test]
    fn results_that_ignore_the_query_are_rejected() {
        let unrelated = vec![SearchHit {
            title: "WhatsApp Web".into(),
            url: "https://web.whatsapp.com".into(),
            snippet: "Log in to WhatsApp".into(),
        }];
        let relevant = vec![SearchHit {
            title: "zerocopy - Rust".into(),
            url: "https://docs.rs/zerocopy".into(),
            snippet: "Safe byte conversions".into(),
        }];

        assert!(!matches_query(&unrelated, "zerocopy rust crate"));
        assert!(matches_query(&relevant, "zerocopy rust crate"));
        // No token long enough to be evidence: accept rather than guess.
        assert!(matches_query(&unrelated, "ai"));
    }

    #[test]
    fn render_limits_rows_and_keeps_each_field() {
        let hits: Vec<SearchHit> = (0..3)
            .map(|index| SearchHit {
                title: format!("title {index}"),
                url: format!("https://example.com/{index}"),
                snippet: format!("snippet {index}"),
            })
            .collect();

        let rendered = render(&hits, 2);

        assert!(rendered.contains("title 0"));
        assert!(rendered.contains("URL: https://example.com/1"));
        assert!(!rendered.contains("title 2"));
        assert_eq!(rendered.lines().count(), 6);
    }

    #[tokio::test]
    async fn empty_query_fails_without_touching_the_network() {
        let result = tool(&["brave"]).execute(&call("   ")).await;

        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("query is required"));
    }

    #[tokio::test]
    async fn unknown_backend_is_named_in_the_failure() {
        let result = tool(&["nope"]).execute(&call("zerocopy crate")).await;

        assert!(!result.success);
        let error = result.error.unwrap_or_default();
        assert!(error.contains("nope: unknown backend"), "{error}");
    }

    #[test]
    fn keyed_backends_declare_their_environment_variable() {
        assert_eq!(Backend::Exa.key_env(), Some("EXA_API_KEY"));
        assert_eq!(Backend::Tavily.key_env(), Some("TAVILY_API_KEY"));
        assert_eq!(Backend::Serper.key_env(), Some("SERPER_API_KEY"));
        assert_eq!(Backend::BraveApi.key_env(), Some("BRAVE_API_KEY"));
        // The scrapers need no key, so they are never skipped for one.
        assert_eq!(Backend::Brave.key_env(), None);
        assert_eq!(Backend::DuckDuckGo.key_env(), None);
    }

    #[test]
    fn configured_key_wins_over_the_environment() {
        let mut config = WebConfig::default();
        config
            .search_keys
            .insert("exa".into(), " from-config ".into());
        let tool = WebSearchTool::new(config);

        assert_eq!(tool.api_key(Backend::Exa).as_deref(), Some("from-config"));
        // No key anywhere: the backend reports itself as unavailable.
        assert_eq!(tool.api_key(Backend::Tavily), None);
        // A keyless backend never consults a key.
        assert_eq!(tool.api_key(Backend::Brave), None);
    }

    #[test]
    fn exa_results_use_highlights_then_text() {
        let value = serde_json::json!({
            "results": [
                { "title": "zerocopy", "url": "https://docs.rs/zerocopy",
                  "highlights": ["Safe byte conversions."] },
                { "title": "Second", "url": "https://example.com/2",
                  "text": "Only text here." },
                { "title": "No url" }
            ]
        });

        let hits = parse_exa(&value);

        assert_eq!(hits.len(), 2, "a result without a url is dropped");
        assert_eq!(hits[0].snippet, "Safe byte conversions.");
        assert_eq!(hits[1].snippet, "Only text here.");
    }

    #[test]
    fn tavily_results_parse() {
        let value = serde_json::json!({
            "results": [{ "title": "T", "url": "https://a.example", "content": "C" }]
        });

        let hits = parse_tavily(&value);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://a.example");
        assert_eq!(hits[0].snippet, "C");
    }

    #[test]
    fn serper_uses_organic_results() {
        let value = serde_json::json!({
            "organic": [{ "title": "T", "link": "https://a.example", "snippet": "S" }]
        });

        let hits = parse_serper(&value);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://a.example");
        assert_eq!(hits[0].snippet, "S");
    }

    #[test]
    fn brave_api_results_parse_under_web_results() {
        let value = serde_json::json!({
            "web": { "results": [{ "title": "T", "url": "https://a.example", "description": "D" }] }
        });

        let hits = parse_brave_api(&value);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].snippet, "D");
    }

    #[test]
    fn a_missing_result_array_is_empty_not_a_panic() {
        let value = serde_json::json!({ "error": "nope" });

        assert!(parse_exa(&value).is_empty());
        assert!(parse_tavily(&value).is_empty());
        assert!(parse_serper(&value).is_empty());
        assert!(parse_brave_api(&value).is_empty());
    }

    #[test]
    fn api_error_bodies_yield_a_readable_message() {
        for body in [
            serde_json::json!({"error": {"message": "Invalid API key"}}),
            serde_json::json!({"detail": {"error": "Unauthorized"}}),
            serde_json::json!({"error": "Invalid API key."}),
            serde_json::json!({"message": "Unauthorized."}),
        ] {
            let message = error_message(&body).unwrap_or_default();
            assert!(!message.is_empty(), "no message from {body}");
        }
    }

    #[tokio::test]
    async fn keyed_backend_without_a_key_is_skipped_with_the_env_var_name() {
        // The environment can supply EXA_API_KEY, and tests share one process, so this
        // only asserts the no-key path when nothing is configured anywhere.
        if std::env::var("EXA_API_KEY").is_ok_and(|key| !key.trim().is_empty()) {
            return;
        }
        // Default backend list starts with keyed providers; with no keys set they are
        // skipped rather than reported as failures.
        let mut config = WebConfig {
            timeout_secs: 1,
            search_backends: vec!["exa".into()],
            ..WebConfig::default()
        };
        config.search_keys.clear();
        let tool = WebSearchTool::new(config);

        let result = tool.execute(&call("zerocopy crate")).await;

        assert!(!result.success);
        let error = result.error.unwrap_or_default();
        assert!(error.contains("exa: no API key"), "{error}");
        assert!(error.contains("EXA_API_KEY"), "{error}");
    }
}
