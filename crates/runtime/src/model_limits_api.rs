//! Optional provider HTTP lookups for context window sizes.
//!
//! - [OpenRouter](https://openrouter.ai/docs/api-reference/models/get-models): public `GET .../models`
//!   with `context_length` per model.
//! - Anthropic: `GET /v1/models` (requires API key); entries may include `max_input_tokens`.
//! - OpenAI: `GET /v1/models` (requires key); `context_window` is present on some responses.
//! - **MiniMax**: the Anthropic-compatible host (`api.minimax.io/anthropic`) does not expose
//!   `/v1/models` (404); context limits stay on the built-in [`crate::model_limits`] table.
//!
//! Successful lookups are cached on disk under the product home for a day, so a run does not
//! wait on the provider's model catalog. Catalog responses are also cached in memory per process
//! (`NCA_CONTEXT_API_CACHE_TTL_SECS`, default `3600`). Use `NCA_SKIP_CONTEXT_API=1` to disable
//! lookups entirely.

use crate::model_limits::ModelLimits;
use nca_common::config::{NcaConfig, ProviderCompatibility, ProviderKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
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
    resolve_model_limits_in(config, model, disk_cache_path().as_deref()).await
}

/// `resolve_model_limits` with the cache file given explicitly, so tests need no environment.
async fn resolve_model_limits_in(
    config: &NcaConfig,
    model: &str,
    cache_file: Option<&Path>,
) -> ModelLimits {
    let static_limits = ModelLimits::for_model(model);
    if !api_lookup_enabled(config) {
        return static_limits;
    }
    let key = limits_cache_key(config, model);
    if let Some(cached) = cache_file.and_then(|path| read_disk_cache(path, &key)) {
        tracing::debug!(model = %model, "context window from the disk cache");
        return cached;
    }
    match query_provider_limits(config, model).await {
        Some(cw) if cw > 0 => {
            tracing::info!(
                model = %model,
                context_window = cw,
                "context window from provider models API"
            );
            let limits = ModelLimits {
                context_window: cw,
                max_output_tokens: static_limits.max_output_tokens,
            };
            if let Some(path) = cache_file {
                write_disk_cache(path, &key, &limits);
            }
            limits
        }
        _ => static_limits,
    }
}

/// Whether this run may ask the provider for a context window at all.
fn api_lookup_enabled(config: &NcaConfig) -> bool {
    std::env::var("NCA_SKIP_CONTEXT_API").ok().as_deref() != Some("1")
        && config.memory.context.auto_detect_context_window
        && config.memory.context.query_provider_models_api
}

/// Asks the active provider for the model's context window. `None` when the provider has no
/// lookup, the key is missing, or the request fails.
async fn query_provider_limits(config: &NcaConfig, model: &str) -> Option<usize> {
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "context API: failed to build HTTP client");
            return None;
        }
    };
    match config.provider.default {
        ProviderKind::OpenRouter => {
            let base = config.provider.openrouter.base_url.trim_end_matches('/');
            let url = format!("{base}/v1/models");
            let key = config.provider.openrouter.resolve_api_key();
            fetch_openrouter_context(&client, &url, model, key.as_deref()).await
        }
        ProviderKind::Anthropic => {
            let Some(key) = config.provider.anthropic.resolve_api_key() else {
                tracing::debug!("context API: anthropic selected but no API key");
                return None;
            };
            let base = config.provider.anthropic.base_url.trim_end_matches('/');
            fetch_anthropic_context(&client, base, &key, model).await
        }
        ProviderKind::OpenAi => {
            let Some(key) = config.provider.openai.resolve_api_key() else {
                tracing::debug!("context API: openai selected but no API key");
                return None;
            };
            let base = config.provider.openai.base_url.trim_end_matches('/');
            fetch_openai_context(&client, base, &key, model).await
        }
        ProviderKind::Custom => {
            let Some(key) = config.provider.custom.resolve_api_key() else {
                tracing::debug!("context API: custom selected but no API key");
                return None;
            };
            let base = config.provider.custom.base_url.trim_end_matches('/');
            match config.provider.custom.compatibility {
                ProviderCompatibility::OpenAi => {
                    fetch_openai_context(&client, base, &key, model).await
                }
                ProviderCompatibility::Anthropic => {
                    fetch_anthropic_context(&client, base, &key, model).await
                }
            }
        }
        ProviderKind::MiniMax => None,
    }
}

/// Cache key for one provider's context window: the provider, its endpoint, and the model.
/// The API key is not part of it.
fn limits_cache_key(config: &NcaConfig, model: &str) -> String {
    let endpoint = match config.provider.default {
        ProviderKind::OpenRouter => config.provider.openrouter.base_url.clone(),
        ProviderKind::Anthropic => config.provider.anthropic.base_url.clone(),
        ProviderKind::OpenAi => config.provider.openai.base_url.clone(),
        ProviderKind::Custom => format!(
            "{}|{:?}",
            config.provider.custom.base_url, config.provider.custom.compatibility
        ),
        ProviderKind::MiniMax => String::new(),
    };
    format!(
        "{:?}|{}|{}",
        config.provider.default,
        endpoint.trim_end_matches('/'),
        model
    )
}

/// How long a resolved context window stays on disk before the provider is asked again.
const DISK_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// One cached lookup, stored so the next run can skip the network call.
#[derive(Debug, Serialize, Deserialize)]
struct DiskEntry {
    context_window: usize,
    max_output_tokens: usize,
    fetched_at: u64,
}

/// The disk cache file: `<product home>/cache/model-limits.json`.
fn disk_cache_path() -> Option<PathBuf> {
    nca_common::config::nca_product_home().map(|home| home.join("cache").join("model-limits.json"))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The cached limits for `key`, if the entry is younger than `DISK_CACHE_TTL`.
fn read_disk_cache(path: &Path, key: &str) -> Option<ModelLimits> {
    let text = std::fs::read_to_string(path).ok()?;
    let map: BTreeMap<String, DiskEntry> = serde_json::from_str(&text).ok()?;
    let entry = map.get(key)?;
    let age = unix_now().saturating_sub(entry.fetched_at);
    (age < DISK_CACHE_TTL.as_secs()).then_some(ModelLimits {
        context_window: entry.context_window,
        max_output_tokens: entry.max_output_tokens,
    })
}

/// Stores `limits` under `key` and drops expired entries. Failures are ignored, because the
/// cache only saves time.
fn write_disk_cache(path: &Path, key: &str, limits: &ModelLimits) {
    let now = unix_now();
    let mut map: BTreeMap<String, DiskEntry> = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    map.retain(|_, entry| now.saturating_sub(entry.fetched_at) < DISK_CACHE_TTL.as_secs());
    map.insert(
        key.to_string(),
        DiskEntry {
            context_window: limits.context_window,
            max_output_tokens: limits.max_output_tokens,
            fetched_at: now,
        },
    );
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Write a temporary file and rename it, so a reader never sees a half-written cache.
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if let Ok(text) = serde_json::to_string(&map)
        && std::fs::write(&tmp, text).is_ok()
    {
        let _ = std::fs::rename(&tmp, path);
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

/// Reads the window from `/v1/models`. OpenAI uses `context_window`; OpenRouter and
/// OpenAI-compatible gateways such as kenari.id use `context_length`.
fn openai_context_from_catalog(value: &serde_json::Value, model: &str) -> Option<usize> {
    let data = value.get("data")?.as_array()?;
    let wanted = model.to_lowercase();
    data.iter()
        .filter(|m| {
            m.get("id")
                .and_then(|id| id.as_str())
                .is_some_and(|id| id.to_lowercase() == wanted)
        })
        .find_map(|m| {
            m.get("context_window")
                .or_else(|| m.get("context_length"))
                .and_then(|x| x.as_u64())
                .filter(|&cw| cw > 0)
                .map(|cw| cw as usize)
        })
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

/// One provider's model ids, stored so a new process can list them at once.
#[derive(Debug, Serialize, Deserialize)]
struct CatalogDiskEntry {
    ids: Vec<String>,
    fetched_at: u64,
}

/// The model-list cache file: `<product home>/cache/model-catalog.json`.
fn catalog_disk_path() -> Option<PathBuf> {
    nca_common::config::nca_product_home().map(|home| home.join("cache").join("model-catalog.json"))
}

fn catalog_disk_key(config: &NcaConfig) -> String {
    limits_cache_key(config, "")
}

fn read_catalog_disk(path: &Path, key: &str) -> Vec<String> {
    let Some(map) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<BTreeMap<String, CatalogDiskEntry>>(&text).ok())
    else {
        return Vec::new();
    };
    match map.get(key) {
        Some(entry) if unix_now().saturating_sub(entry.fetched_at) < DISK_CACHE_TTL.as_secs() => {
            entry.ids.clone()
        }
        _ => Vec::new(),
    }
}

fn write_catalog_disk(path: &Path, key: &str, ids: &[String]) {
    let now = unix_now();
    let mut map: BTreeMap<String, CatalogDiskEntry> = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    map.retain(|_, entry| now.saturating_sub(entry.fetched_at) < DISK_CACHE_TTL.as_secs());
    map.insert(
        key.to_string(),
        CatalogDiskEntry {
            ids: ids.to_vec(),
            fetched_at: now,
        },
    );
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if let Ok(text) = serde_json::to_string(&map)
        && std::fs::write(&tmp, text).is_ok()
    {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// The active provider's model ids saved by an earlier fetch (up to a day
/// old), without touching the network. Empty when nothing is saved.
pub fn cached_provider_model_ids(config: &NcaConfig) -> Vec<String> {
    catalog_disk_path()
        .map(|path| read_catalog_disk(&path, &catalog_disk_key(config)))
        .unwrap_or_default()
}

/// Fetch available model IDs from the active provider's API.
/// Returns a sorted list of model ID strings. Uses the same in-memory cache as
/// context-window lookups, and saves the list on disk for the next process.
pub async fn fetch_provider_model_ids(config: &NcaConfig) -> Vec<String> {
    let ids = fetch_provider_model_ids_live(config).await;
    if !ids.is_empty()
        && config.provider.default != ProviderKind::MiniMax
        && let Some(path) = catalog_disk_path()
    {
        write_catalog_disk(&path, &catalog_disk_key(config), &ids);
    }
    ids
}

async fn fetch_provider_model_ids_live(config: &NcaConfig) -> Vec<String> {
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

/// Sorted ids of the catalog's models that can run an agent turn.
///
/// Gateways such as kenari.id describe each model: `endpoints` lists what it
/// serves (`chat`, `images`, `music`, ...) and `tool_call` says whether it can
/// call tools. Models that declare no chat endpoint, or no tool support, are
/// skipped. Plain OpenAI catalogs carry neither field, so nothing is dropped.
fn chat_model_ids(catalog: &serde_json::Value) -> Vec<String> {
    let Some(models) = catalog.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = models
        .iter()
        .filter(|m| {
            let serves_chat = m
                .get("endpoints")
                .and_then(|e| e.as_array())
                .is_none_or(|endpoints| endpoints.iter().any(|e| e.as_str() == Some("chat")));
            let no_tools = m.get("tool_call").and_then(|t| t.as_bool()) == Some(false);
            serves_chat && !no_tools
        })
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    ids.sort();
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
        {
            return chat_model_ids(entry.value.as_ref());
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
    let ids = chat_model_ids(value.as_ref());
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

    #[test]
    fn openai_compatible_catalog_reads_context_length_and_skips_malformed_entries() {
        let v: serde_json::Value = serde_json::json!({
            "data": [
                { "name": "no id here", "context_length": 1 },
                { "id": "muse-spark-1-3-contributor:free", "context_length": 1048576 },
                { "id": "zero-window", "context_length": 0 }
            ]
        });
        assert_eq!(
            openai_context_from_catalog(&v, "muse-spark-1-3-contributor:free"),
            Some(1_048_576)
        );
        assert_eq!(openai_context_from_catalog(&v, "zero-window"), None);
    }
}

#[cfg(test)]
mod disk_cache_tests {
    use super::*;

    #[test]
    fn disk_cache_keeps_fresh_entries_and_drops_expired_ones() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("cache").join("model-limits.json");
        let key = "custom|https://kenari.id|m";
        let limits = ModelLimits {
            context_window: 1_000_000,
            max_output_tokens: 8192,
        };
        write_disk_cache(&path, key, &limits);
        let back = read_disk_cache(&path, key).expect("fresh entry");
        assert_eq!(back.context_window, 1_000_000);
        assert_eq!(back.max_output_tokens, 8192);
        assert!(read_disk_cache(&path, "other").is_none());

        // An entry older than the TTL is ignored.
        let mut map: BTreeMap<String, DiskEntry> =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
        map.get_mut(key).expect("entry").fetched_at = 0;
        std::fs::write(&path, serde_json::to_string(&map).expect("json")).expect("write");
        assert!(read_disk_cache(&path, key).is_none());
    }

    #[tokio::test]
    async fn a_cached_context_window_is_used_without_asking_the_provider() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("model-limits.json");
        let mut config = NcaConfig::default();
        config.provider.default = ProviderKind::Custom;
        config.provider.custom.base_url = "http://127.0.0.1:9".into();
        config.provider.custom.api_key = Some("test-key".into());
        let key = limits_cache_key(&config, "test-model");
        write_disk_cache(
            &path,
            &key,
            &ModelLimits {
                context_window: 777_000,
                max_output_tokens: 4096,
            },
        );
        // Nothing listens on port 9, so a network call would fall back to the static table.
        let limits = resolve_model_limits_in(&config, "test-model", Some(&path)).await;
        assert_eq!(limits.context_window, 777_000);
    }

    #[test]
    fn catalog_lists_only_models_that_can_chat_with_tools() {
        let catalog = serde_json::json!({"data": [
            {"id": "deepseek-v4-1-flash", "endpoints": ["chat"], "tool_call": true},
            {"id": "lyria-3-5", "endpoints": ["music"]},
            {"id": "nano-banana-2", "endpoints": ["images"], "tool_call": false},
            {"id": "chat-without-tools", "endpoints": ["chat"], "tool_call": false},
            {"id": "gpt-4o"}
        ]});
        assert_eq!(chat_model_ids(&catalog), ["deepseek-v4-1-flash", "gpt-4o"]);
    }

    #[test]
    fn model_catalog_round_trips_through_the_disk_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cache").join("model-catalog.json");
        let ids = vec!["deepseek-v4-1-flash".to_string(), "minimax-m3".to_string()];
        assert!(read_catalog_disk(&path, "k").is_empty());
        write_catalog_disk(&path, "k", &ids);
        assert_eq!(read_catalog_disk(&path, "k"), ids);
        assert!(read_catalog_disk(&path, "other").is_empty());
    }
}
