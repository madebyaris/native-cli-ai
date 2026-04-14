//! Optional provider HTTP lookups for context window sizes.
//!
//! - [OpenRouter](https://openrouter.ai/docs/api-reference/models/get-models): public `GET .../models`
//!   with `context_length` per model.
//! - Anthropic: `GET /v1/models` (requires API key); entries may include `max_input_tokens`.
//! - OpenAI: `GET /v1/models` (requires key); `context_window` is present on some responses.
//! - **MiniMax**: the Anthropic-compatible host (`api.minimax.io/anthropic`) does not expose
//!   `/v1/models` (404); context limits stay on the built-in [`crate::model_limits`] table.
//!
//! Successful catalog responses are cached in memory per process. Tune with
//! `NCA_CONTEXT_API_CACHE_TTL_SECS` (default `3600`). Use `NCA_SKIP_CONTEXT_API=1` to disable
//! lookups entirely.

use crate::model_limits::ModelLimits;
use nca_common::config::{NcaConfig, ProviderCompatibility, ProviderKind};
use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const HTTP_TIMEOUT_SECS: u64 = 12;

fn catalog_cache_ttl() -> Duration {
    std::env::var("NCA_CONTEXT_API_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(3600))
}

fn api_key_tag(secret: &str) -> u64 {
    let mut h = DefaultHasher::new();
    secret.hash(&mut h);
    h.finish()
}

fn cache_stale(fetched_at: Instant, ttl: Duration) -> bool {
    fetched_at.elapsed() >= ttl
}

// --- OpenRouter ---

struct OpenRouterCatalogEntry {
    url: String,
    fetched_at: Instant,
    models: Arc<Vec<OpenRouterModel>>,
}

fn openrouter_catalog_cache() -> &'static Mutex<Option<OpenRouterCatalogEntry>> {
    static CELL: OnceLock<Mutex<Option<OpenRouterCatalogEntry>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

// --- Anthropic ---

struct AnthropicCatalogEntry {
    cache_key: String,
    fetched_at: Instant,
    models: Arc<Vec<AnthropicModel>>,
}

fn anthropic_catalog_cache() -> &'static Mutex<Option<AnthropicCatalogEntry>> {
    static CELL: OnceLock<Mutex<Option<AnthropicCatalogEntry>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

// --- OpenAI ---

struct OpenAiCatalogEntry {
    cache_key: String,
    fetched_at: Instant,
    value: Arc<serde_json::Value>,
}

fn openai_catalog_cache() -> &'static Mutex<Option<OpenAiCatalogEntry>> {
    static CELL: OnceLock<Mutex<Option<OpenAiCatalogEntry>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

pub async fn resolve_model_limits(config: &NcaConfig, model: &str) -> ModelLimits {
    let static_limits = ModelLimits::for_model(model);

    if std::env::var("NCA_SKIP_CONTEXT_API").ok().as_deref() == Some("1") {
        return static_limits;
    }

    if !config.memory.context.auto_detect_context_window
        || !config.memory.context.query_provider_models_api
    {
        return static_limits;
    }

    let client = match http_client() {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "context API: failed to build HTTP client");
            return static_limits;
        }
    };

    let from_api = match config.provider.default {
        ProviderKind::OpenRouter => {
            let base = config.provider.openrouter.base_url.trim_end_matches('/');
            let url = format!("{base}/v1/models");
            let key = config.provider.openrouter.resolve_api_key();
            fetch_openrouter_context(&client, &url, model, key.as_deref()).await
        }
        ProviderKind::Anthropic => {
            let key = match config.provider.anthropic.resolve_api_key() {
                Some(k) => k,
                None => {
                    tracing::debug!("context API: anthropic selected but no API key");
                    return static_limits;
                }
            };
            let base = config.provider.anthropic.base_url.trim_end_matches('/');
            fetch_anthropic_context(&client, base, &key, model).await
        }
        ProviderKind::OpenAi => {
            let key = match config.provider.openai.resolve_api_key() {
                Some(k) => k,
                None => {
                    tracing::debug!("context API: openai selected but no API key");
                    return static_limits;
                }
            };
            let base = config.provider.openai.base_url.trim_end_matches('/');
            fetch_openai_context(&client, base, &key, model).await
        }
        ProviderKind::Custom => {
            let key = match config.provider.custom.resolve_api_key() {
                Some(k) => k,
                None => {
                    tracing::debug!("context API: custom selected but no API key");
                    return static_limits;
                }
            };
            let base = config.provider.custom.base_url.trim_end_matches('/');
            match config.provider.custom.compatibility {
                ProviderCompatibility::OpenAi => fetch_openai_context(&client, base, &key, model).await,
                ProviderCompatibility::Anthropic => {
                    fetch_anthropic_context(&client, base, &key, model).await
                }
            }
        }
        ProviderKind::MiniMax => None,
    };

    match from_api {
        Some(cw) if cw > 0 => {
            tracing::info!(
                model = %model,
                context_window = cw,
                "context window from provider models API"
            );
            ModelLimits {
                context_window: cw,
                max_output_tokens: static_limits.max_output_tokens,
            }
        }
        _ => static_limits,
    }
}

fn http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .user_agent(concat!(
            "nca/",
            env!("CARGO_PKG_VERSION"),
            " (context-window lookup)"
        ))
        .build()
}

#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModel {
    id: String,
    context_length: Option<u64>,
}

async fn fetch_openrouter_context(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    api_key: Option<&str>,
) -> Option<usize> {
    let ttl = catalog_cache_ttl();
    {
        let guard = openrouter_catalog_cache().lock().ok()?;
        if let Some(entry) = guard.as_ref()
            && entry.url == url
            && !cache_stale(entry.fetched_at, ttl)
        {
            tracing::debug!(url = %url, "openrouter models catalog cache hit");
            return pick_openrouter(entry.models.as_ref(), model)
                .and_then(|m| m.context_length)
                .map(|n| n as usize);
        }
    }

    let mut req = client.get(url);
    if let Some(k) = api_key.filter(|s| !s.is_empty()) {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        tracing::debug!(status = %resp.status(), url = %url, "openrouter models request failed");
        return None;
    }
    let body: OpenRouterModelsResponse = resp.json().await.ok()?;
    let models = Arc::new(body.data);
    {
        if let Ok(mut guard) = openrouter_catalog_cache().lock() {
            *guard = Some(OpenRouterCatalogEntry {
                url: url.to_string(),
                fetched_at: Instant::now(),
                models: Arc::clone(&models),
            });
        }
    }
    pick_openrouter(models.as_ref(), model)
        .and_then(|m| m.context_length)
        .map(|n| n as usize)
}

fn pick_openrouter<'a>(models: &'a [OpenRouterModel], wanted: &str) -> Option<&'a OpenRouterModel> {
    let w = wanted.to_lowercase();
    models
        .iter()
        .find(|m| m.id.to_lowercase() == w)
        .or_else(|| {
            models.iter().find(|m| {
                let id = m.id.to_lowercase();
                id.ends_with(&format!("/{w}"))
            })
        })
}

#[derive(Debug, Deserialize)]
struct AnthropicModelsPage {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct AnthropicModel {
    id: String,
    max_input_tokens: Option<u64>,
    /// Present on some API versions; reserved for future output-cap hints.
    #[serde(default)]
    #[allow(dead_code)]
    max_tokens: Option<u64>,
}

async fn fetch_anthropic_context(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    model: &str,
) -> Option<usize> {
    let ttl = catalog_cache_ttl();
    let cache_key = format!("anthropic|{}|{:x}", base, api_key_tag(api_key));
    {
        let guard = anthropic_catalog_cache().lock().ok()?;
        if let Some(entry) = guard.as_ref()
            && entry.cache_key == cache_key
            && !cache_stale(entry.fetched_at, ttl)
        {
            tracing::debug!("anthropic models catalog cache hit");
            return pick_anthropic(entry.models.as_ref(), model)
                .and_then(|m| m.max_input_tokens)
                .map(|n| n as usize);
        }
    }

    let mut all: Vec<AnthropicModel> = Vec::new();
    let mut after_id: Option<String> = None;

    loop {
        let mut url = format!("{base}/v1/models?limit=100");
        if let Some(ref id) = after_id {
            url.push_str("&after_id=");
            url.push_str(id);
        }

        let resp = client
            .get(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            tracing::debug!(status = %resp.status(), url = %url, "anthropic models request failed");
            return None;
        }

        let page: AnthropicModelsPage = resp.json().await.ok()?;
        if page.data.is_empty() {
            break;
        }

        let cursor = page.data.last().map(|m| m.id.clone());
        all.extend(page.data);

        if !page.has_more {
            break;
        }
        after_id = cursor;
    }

    let models = Arc::new(all);
    {
        if let Ok(mut guard) = anthropic_catalog_cache().lock() {
            *guard = Some(AnthropicCatalogEntry {
                cache_key,
                fetched_at: Instant::now(),
                models: Arc::clone(&models),
            });
        }
    }
    pick_anthropic(models.as_ref(), model)
        .and_then(|m| m.max_input_tokens)
        .map(|n| n as usize)
}

fn pick_anthropic<'a>(models: &'a [AnthropicModel], wanted: &str) -> Option<&'a AnthropicModel> {
    let w = wanted.to_lowercase();
    if let Some(m) = models.iter().find(|m| m.id.to_lowercase() == w) {
        return Some(m);
    }
    models.iter().find(|m| {
        let id = m.id.to_lowercase();
        id.starts_with(&w) && (id.len() == w.len() || id.as_bytes().get(w.len()) == Some(&b'-'))
    })
}

fn openai_context_from_catalog(value: &serde_json::Value, model: &str) -> Option<usize> {
    let data = value.get("data")?.as_array()?;
    let w = model.to_lowercase();
    for m in data {
        let id = m.get("id")?.as_str()?.to_lowercase();
        if id != w {
            continue;
        }
        if let Some(cw) = m.get("context_window").and_then(|x| x.as_u64()) {
            return Some(cw as usize);
        }
    }
    None
}

async fn fetch_openai_context(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    model: &str,
) -> Option<usize> {
    let ttl = catalog_cache_ttl();
    let cache_key = format!("openai|{}|{:x}", base, api_key_tag(api_key));
    {
        let guard = openai_catalog_cache().lock().ok()?;
        if let Some(entry) = guard.as_ref()
            && entry.cache_key == cache_key
            && !cache_stale(entry.fetched_at, ttl)
        {
            tracing::debug!("openai models catalog cache hit");
            return openai_context_from_catalog(entry.value.as_ref(), model);
        }
    }

    let url = format!("{base}/v1/models");
    let resp = client.get(&url).bearer_auth(api_key).send().await.ok()?;
    if !resp.status().is_success() {
        tracing::debug!(status = %resp.status(), url = %url, "openai models request failed");
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    let value = Arc::new(v);
    {
        if let Ok(mut guard) = openai_catalog_cache().lock() {
            *guard = Some(OpenAiCatalogEntry {
                cache_key,
                fetched_at: Instant::now(),
                value: Arc::clone(&value),
            });
        }
    }
    openai_context_from_catalog(value.as_ref(), model)
}

/// Fetch available model IDs from the active provider's API.
/// Returns a sorted list of model ID strings. Uses the same cache as context-window lookups.
pub async fn fetch_provider_model_ids(config: &NcaConfig) -> Vec<String> {
    if !config.memory.context.query_provider_models_api {
        return Vec::new();
    }
    if std::env::var("NCA_SKIP_CONTEXT_API").ok().as_deref() == Some("1") {
        return Vec::new();
    }
    let client = match http_client() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    match config.provider.default {
        ProviderKind::OpenRouter => fetch_openrouter_model_ids(&client, config).await,
        ProviderKind::Anthropic => fetch_anthropic_model_ids(&client, config).await,
        ProviderKind::OpenAi => fetch_openai_model_ids(&client, config).await,
        ProviderKind::Custom => fetch_custom_model_ids(&client, config).await,
        ProviderKind::MiniMax => vec!["MiniMax-M2.5".into(), "MiniMax-M2.7".into()],
    }
}

async fn fetch_custom_model_ids(client: &reqwest::Client, config: &NcaConfig) -> Vec<String> {
    let key = match config.provider.custom.resolve_api_key() {
        Some(k) => k,
        None => return Vec::new(),
    };
    if config.provider.custom.base_url.trim().is_empty() {
        return Vec::new();
    }

    let mut shadow = config.clone();
    match config.provider.custom.compatibility {
        ProviderCompatibility::OpenAi => {
            shadow.provider.openai.api_key = Some(key);
            shadow.provider.openai.base_url = config.provider.custom.base_url.clone();
            fetch_openai_model_ids(client, &shadow).await
        }
        ProviderCompatibility::Anthropic => {
            shadow.provider.anthropic.api_key = Some(key);
            shadow.provider.anthropic.base_url = config.provider.custom.base_url.clone();
            fetch_anthropic_model_ids(client, &shadow).await
        }
    }
}

async fn fetch_openrouter_model_ids(client: &reqwest::Client, config: &NcaConfig) -> Vec<String> {
    let base = config.provider.openrouter.base_url.trim_end_matches('/');
    let url = format!("{base}/v1/models");
    let key = config.provider.openrouter.resolve_api_key();
    let ttl = catalog_cache_ttl();

    // Check cache first
    {
        let guard = openrouter_catalog_cache().lock().ok();
        if let Some(Some(entry)) = guard.as_ref().map(|g| g.as_ref())
            && entry.url == url
            && !cache_stale(entry.fetched_at, ttl)
        {
            let mut ids: Vec<String> = entry.models.iter().map(|m| m.id.clone()).collect();
            ids.sort();
            return ids;
        }
    }

    let mut req = client.get(&url);
    if let Some(k) = key.as_deref().filter(|s| !s.is_empty()) {
        req = req.bearer_auth(k);
    }
    let resp = match req.send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    let body: OpenRouterModelsResponse = match resp.json().await {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let models = Arc::new(body.data);
    let mut ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
    ids.sort();
    {
        if let Ok(mut guard) = openrouter_catalog_cache().lock() {
            *guard = Some(OpenRouterCatalogEntry {
                url,
                fetched_at: Instant::now(),
                models,
            });
        }
    }
    ids
}

async fn fetch_anthropic_model_ids(client: &reqwest::Client, config: &NcaConfig) -> Vec<String> {
    let key = match config.provider.anthropic.resolve_api_key() {
        Some(k) => k,
        None => return Vec::new(),
    };
    let base = config.provider.anthropic.base_url.trim_end_matches('/');
    let ttl = catalog_cache_ttl();
    let cache_key = format!("anthropic|{}|{:x}", base, api_key_tag(&key));

    {
        let guard = anthropic_catalog_cache().lock().ok();
        if let Some(Some(entry)) = guard.as_ref().map(|g| g.as_ref())
            && entry.cache_key == cache_key
            && !cache_stale(entry.fetched_at, ttl)
        {
            let mut ids: Vec<String> = entry.models.iter().map(|m| m.id.clone()).collect();
            ids.sort();
            return ids;
        }
    }

    let mut all: Vec<AnthropicModel> = Vec::new();
    let mut after_id: Option<String> = None;
    let mut completed = true;
    loop {
        let mut url = format!("{base}/v1/models?limit=100");
        if let Some(ref id) = after_id {
            url.push_str("&after_id=");
            url.push_str(id);
        }
        let resp = match client
            .get(&url)
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => {
                completed = false;
                break;
            }
        };
        let page: AnthropicModelsPage = match resp.json().await {
            Ok(p) => p,
            Err(_) => {
                completed = false;
                break;
            }
        };
        if page.data.is_empty() {
            break;
        }
        let cursor = page.data.last().map(|m| m.id.clone());
        all.extend(page.data);
        if !page.has_more {
            break;
        }
        after_id = cursor;
    }

    if !completed && all.is_empty() {
        return Vec::new();
    }

    let models = Arc::new(all);
    let mut ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
    ids.sort();
    if completed && let Ok(mut guard) = anthropic_catalog_cache().lock() {
        *guard = Some(AnthropicCatalogEntry {
            cache_key,
            fetched_at: Instant::now(),
            models,
        });
    }
    ids
}

async fn fetch_openai_model_ids(client: &reqwest::Client, config: &NcaConfig) -> Vec<String> {
    let key = match config.provider.openai.resolve_api_key() {
        Some(k) => k,
        None => return Vec::new(),
    };
    let base = config.provider.openai.base_url.trim_end_matches('/');
    let ttl = catalog_cache_ttl();
    let cache_key = format!("openai|{}|{:x}", base, api_key_tag(&key));

    {
        let guard = openai_catalog_cache().lock().ok();
        if let Some(Some(entry)) = guard.as_ref().map(|g| g.as_ref())
            && entry.cache_key == cache_key
            && !cache_stale(entry.fetched_at, ttl)
            && let Some(arr) = entry.value.get("data").and_then(|d| d.as_array())
        {
            let mut ids: Vec<String> = arr
                .iter()
                .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect();
            ids.sort();
            return ids;
        }
    }

    let url = format!("{base}/v1/models");
    let resp = match client.get(&url).bearer_auth(&key).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    let v: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let value = Arc::new(v);
    let mut ids = Vec::new();
    if let Some(arr) = value.get("data").and_then(|d| d.as_array()) {
        ids = arr
            .iter()
            .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(String::from))
            .collect();
        ids.sort();
    }
    {
        if let Ok(mut guard) = openai_catalog_cache().lock() {
            *guard = Some(OpenAiCatalogEntry {
                cache_key,
                fetched_at: Instant::now(),
                value,
            });
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_pick_exact() {
        let models = vec![
            OpenRouterModel {
                id: "openai/gpt-4o".into(),
                context_length: Some(128_000),
            },
            OpenRouterModel {
                id: "other/x".into(),
                context_length: Some(8_000),
            },
        ];
        let m = pick_openrouter(&models, "openai/gpt-4o").unwrap();
        assert_eq!(m.context_length, Some(128_000));
    }

    #[test]
    fn anthropic_pick_prefix() {
        let models = vec![AnthropicModel {
            id: "claude-3-5-sonnet-20241022".into(),
            max_input_tokens: Some(200_000),
            max_tokens: Some(8192),
        }];
        let m = pick_anthropic(&models, "claude-3-5-sonnet").unwrap();
        assert_eq!(m.max_input_tokens, Some(200_000));
    }

    #[test]
    fn openai_parse_context_window_from_cached_json() {
        let v: serde_json::Value = serde_json::json!({
            "data": [
                { "id": "gpt-4o", "context_window": 128000 }
            ]
        });
        assert_eq!(openai_context_from_catalog(&v, "gpt-4o"), Some(128_000));
        assert_eq!(openai_context_from_catalog(&v, "gpt-4o-mini"), None);
    }
}
