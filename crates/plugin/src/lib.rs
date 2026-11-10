//! WASM plugin factory for nca.
//!
//! * [`install`]: `nca add <git-url>` and friends (clone, validate, approve, install).
//! * [`registry`]: what is installed, with hashes pinned at approval time.
//! * [`host`]: sandboxed wasmtime runtime and hook chain.
//! * this module: the process-wide hook API called from core, runtime and TUI.
//!
//! Every hook is a no-op when no plugin is installed.

pub mod error;
pub mod host;
pub mod install;
pub mod manage;
pub mod manifest;
pub mod registry;
pub mod types;

use std::path::Path;
use std::sync::OnceLock;

use nca_common::config::{NcaConfig, ProviderKind};

pub use error::PluginError;
pub use host::PluginSet;
pub use types::{
    AuthRecovery, Credential, LoginChallenge, SpawnDecision, SpawnRequest, ThemeEntries,
    ToolDecision,
};

static GLOBAL: OnceLock<PluginSet> = OnceLock::new();

/// Load installed plugins for this process. Call once at startup, after the
/// workspace is known. Later calls are ignored.
pub fn init(workspace: &Path) {
    GLOBAL.get_or_init(|| match registry::Registry::load() {
        Ok(reg) if !reg.plugins.is_empty() => PluginSet::load(&reg.plugins, workspace),
        Ok(_) => PluginSet::empty(),
        Err(e) => {
            tracing::warn!("plugins disabled: {e}");
            PluginSet::empty()
        }
    });
}

fn active() -> Option<&'static PluginSet> {
    GLOBAL.get().filter(|s| !s.is_empty())
}

/// Names of loaded plugins, in chain order.
pub fn loaded_names() -> Vec<String> {
    GLOBAL.get().map(|s| s.names().to_vec()).unwrap_or_default()
}

/// The provider id plugins see (`anthropic`, `openai`, ...).
pub fn provider_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::MiniMax => "minimax",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenRouter => "openrouter",
        ProviderKind::Alibaba => "alibaba",
        ProviderKind::Custom => "custom",
    }
}

fn configured_credential(config: &NcaConfig, kind: ProviderKind) -> Credential {
    let p = &config.provider;
    let (api_key, base_url) = match kind {
        ProviderKind::MiniMax => (p.minimax.resolve_api_key(), &p.minimax.base_url),
        ProviderKind::OpenAi => (p.openai.resolve_api_key(), &p.openai.base_url),
        ProviderKind::Anthropic => (p.anthropic.resolve_api_key(), &p.anthropic.base_url),
        ProviderKind::OpenRouter => (p.openrouter.resolve_api_key(), &p.openrouter.base_url),
        ProviderKind::Alibaba => (p.alibaba.resolve_api_key(), &p.alibaba.base_url),
        ProviderKind::Custom => (p.custom.resolve_api_key(), &p.custom.base_url),
    };
    Credential {
        api_key,
        base_url: Some(base_url.clone()),
    }
}

/// Auth hook. Lets plugins replace the API key / base URL of the default
/// provider before it is built. Fail-closed: a plugin error is returned.
pub fn apply_auth(config: &mut NcaConfig) -> Result<(), PluginError> {
    let Some(set) = active() else { return Ok(()) };
    let kind = config.provider.default;
    let default = configured_credential(config, kind);
    let resolved = set.resolve_credential(provider_name(kind), Some(default))?;
    if let Some(cred) = resolved {
        if let Some(key) = cred.api_key {
            config.set_provider_api_key(kind, key);
        }
        if let Some(url) = cred.base_url {
            config.set_provider_base_url(kind, url);
        }
    }
    Ok(())
}

/// What an installed auth plugin says about `kind`: `None` = no opinion (no key from a
/// plugin), `Some(Ok(()))` = it supplies a key, `Some(Err(msg))` = it tried and failed
/// (for example an expired subscription token). UI use only.
pub fn plugin_key_status(config: &NcaConfig, kind: ProviderKind) -> Option<Result<(), String>> {
    let set = active()?;
    let default = configured_credential(config, kind);
    match set.resolve_credential(provider_name(kind), Some(default.clone())) {
        Ok(Some(Credential {
            api_key: Some(key), ..
        })) if default.api_key.as_deref() != Some(key.as_str()) => Some(Ok(())),
        Ok(_) => None,
        Err(e) => Some(Err(e.to_string())),
    }
}

/// How long [`plugin_supplies_key`] reuses an answer. A plugin call can touch the network
/// (a token refresh), and UI code asks on every redraw of a picker.
const KEY_STATUS_TTL: std::time::Duration = std::time::Duration::from_secs(30);

type KeyStatusCache =
    std::sync::Mutex<std::collections::HashMap<&'static str, (std::time::Instant, bool)>>;

fn key_status_cache() -> &'static KeyStatusCache {
    static CACHE: OnceLock<KeyStatusCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn forget_key_status() {
    if let Ok(mut cache) = key_status_cache().lock() {
        cache.clear();
    }
}

/// Whether an installed auth plugin currently supplies an API key for `kind`. Cached for a
/// short time so it is safe to call while drawing; sign-in and auth failures clear the cache.
pub fn plugin_supplies_key(config: &NcaConfig, kind: ProviderKind) -> bool {
    if active().is_none() {
        return false;
    }
    let name = provider_name(kind);
    if let Ok(cache) = key_status_cache().lock()
        && let Some((at, supplied)) = cache.get(name)
        && at.elapsed() < KEY_STATUS_TTL
    {
        return *supplied;
    }
    let supplied = matches!(plugin_key_status(config, kind), Some(Ok(())));
    if let Ok(mut cache) = key_status_cache().lock() {
        cache.insert(name, (std::time::Instant::now(), supplied));
    }
    supplied
}

/// Plugins that can sign a provider in, as `(plugin, providers)`. UI use only.
pub fn login_plugins() -> Vec<(String, Vec<String>)> {
    active().map(PluginSet::login_plugins).unwrap_or_default()
}

/// Start a sign-in for `kind` (for example `/login`).
pub fn begin_login(kind: ProviderKind) -> Result<LoginChallenge, PluginError> {
    let set = active().ok_or_else(|| PluginError::Runtime {
        plugin: "login".into(),
        message: "no plugins are loaded".into(),
    })?;
    set.begin_login(provider_name(kind))
}

/// Finish a sign-in with the code the user pasted (or the callback query string).
pub fn complete_login(kind: ProviderKind, input: &str) -> Result<(), PluginError> {
    let set = active().ok_or_else(|| PluginError::Runtime {
        plugin: "login".into(),
        message: "no plugins are loaded".into(),
    })?;
    forget_key_status();
    set.complete_login(provider_name(kind), input)
}

/// The provider answered 401/403 to a credential a plugin supplied. Lets the
/// plugin refresh it. Network failures are not auth failures: do not call this for them.
pub fn on_auth_failure(kind: ProviderKind, status: u16) -> AuthRecovery {
    forget_key_status();
    match active() {
        Some(set) => set.on_auth_failure(provider_name(kind), status),
        None => AuthRecovery::NoPlugin,
    }
}

/// Subagent hook. `Deny` must stop the spawn; `Allow` carries the request to use.
/// Fail-closed: a plugin error denies.
pub fn before_spawn(request: SpawnRequest) -> SpawnDecision {
    let Some(set) = active() else {
        return SpawnDecision::Allow(request);
    };
    set.before_spawn(request)
        .unwrap_or_else(|e| SpawnDecision::Deny(e.to_string()))
}

/// Agent hook: system prompt rewriting. Fail-open: errors keep the prompt.
pub fn transform_system_prompt(prompt: String) -> String {
    match active() {
        Some(set) => set.transform_system_prompt(prompt),
        None => prompt,
    }
}

/// Agent hook: policy check before each tool call. Fail-closed.
pub fn before_tool_use(tool: &str, input_json: &str) -> ToolDecision {
    match active() {
        Some(set) => set.before_tool_use(tool, input_json),
        None => ToolDecision::Allow,
    }
}

/// Cosmetic hook. `defaults` are the built-in `(token, "#rrggbb")` pairs; the
/// result is the effective theme. Fail-open: errors return `defaults`.
pub fn theme(defaults: ThemeEntries) -> ThemeEntries {
    let Some(set) = active() else { return defaults };
    match set.theme(defaults.clone()) {
        // A plugin may return only the tokens it changes: anything it omits keeps its default.
        Ok(returned) => merge_theme(defaults, returned),
        Err(e) => {
            tracing::warn!("{e}; using default theme");
            defaults
        }
    }
}

fn merge_theme(defaults: ThemeEntries, returned: ThemeEntries) -> ThemeEntries {
    let mut out = defaults;
    for (token, value) in returned {
        match out.iter_mut().find(|(t, _)| *t == token) {
            Some(slot) => slot.1 = value,
            None => out.push((token, value)),
        }
    }
    out
}

#[cfg(test)]
mod theme_tests {
    use super::merge_theme;

    #[test]
    fn omitted_tokens_keep_their_default() {
        let defaults = vec![("bg".into(), "#000".into()), ("text".into(), "#fff".into())];
        let merged = merge_theme(defaults, vec![("text".into(), "#eee".into())]);
        assert_eq!(
            merged,
            vec![("bg".into(), "#000".into()), ("text".into(), "#eee".into())]
        );
    }
}

#[cfg(test)]
mod e2e_tests;
