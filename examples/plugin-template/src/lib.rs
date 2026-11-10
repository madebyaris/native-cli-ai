//! Template nca plugin. Build with `./build.sh`, which produces `plugin.wasm`.
//!
//! Targets the `plugin-login` world: the four hooks plus the optional `login`
//! interface (sign-in, refresh on 401/403, re-login). Plugins that do not own a
//! credential should use `world: "plugin"` and leave `login` out of the manifest.

use std::path::PathBuf;

/// Where this plugin keeps its own token (needs `fs_write = ["$NCA_PLUGIN_DATA"]`).
fn data_file(name: &str) -> PathBuf {
    PathBuf::from(std::env::var("NCA_PLUGIN_DATA").unwrap_or_else(|_| "/data".into())).join(name)
}

wit_bindgen::generate!({
    path: "../../crates/plugin/wit",
    world: "plugin-login",
});

use exports::nca::plugin::login::Challenge;
use exports::nca::plugin::{agent, auth, cosmetic, login, subagent};
use nca::plugin::host;
use nca::plugin::types::{Credential, LogLevel, SpawnDecision, SpawnRequest, Theme, ToolDecision};

struct Example;

impl auth::Guest for Example {
    /// Prefer `EXAMPLE_API_KEY` for the anthropic provider, otherwise defer to core.
    fn resolve_credential(provider: String) -> Result<Option<Credential>, String> {
        if provider == "anthropic"
            && let Some(key) = host::env_get("EXAMPLE_API_KEY").filter(|k| !k.trim().is_empty())
        {
            host::log(LogLevel::Info, "using EXAMPLE_API_KEY");
            return Ok(Some(Credential {
                api_key: Some(key),
                base_url: None,
            }));
        }
        // A token saved by `login` comes next; otherwise defer to core.
        if provider == "anthropic"
            && let Ok(token) = std::fs::read_to_string(data_file("token"))
            && !token.trim().is_empty()
        {
            return Ok(Some(Credential {
                api_key: Some(token.trim().to_string()),
                base_url: None,
            }));
        }
        host::next_credential(&provider)
    }
}

impl login::Guest for Example {
    fn providers() -> Vec<String> {
        vec!["anthropic".to_string()]
    }

    /// Hand nca a URL to open. A real plugin builds an OAuth authorize URL with
    /// PKCE here and keeps the verifier in its data directory until `complete_login`.
    fn begin_login(_provider: String) -> Result<Challenge, String> {
        Ok(Challenge {
            url: host::config_get("login_url").unwrap_or_else(|| "https://example.com/login".into()),
            instructions: "Approve access, then paste the code the page shows.".into(),
            mode: "paste-code".into(),
        })
    }

    /// Exchange the pasted code for a token and store it (a real plugin POSTs to the
    /// token endpoint with `host::http_request`, and also stores the refresh token).
    fn complete_login(_provider: String, input: String) -> Result<(), String> {
        let code = input.trim();
        if !code.starts_with("code-") {
            return Err("that code is not valid; start again with /login".into());
        }
        std::fs::write(data_file("token"), format!("token-{code}")).map_err(|e| e.to_string())?;
        std::fs::write(data_file("refresh"), "refresh-1").map_err(|e| e.to_string())
    }

    /// The provider answered 401/403. Refresh and return the new token, or return
    /// `None` so nca asks the user to sign in again. Never call this for network errors.
    fn on_auth_failure(_provider: String, _status: u16) -> Result<Option<String>, String> {
        let Ok(refresh) = std::fs::read_to_string(data_file("refresh")) else {
            return Ok(None);
        };
        // The example's refresh token works once; a real one is exchanged over HTTP.
        let _ = std::fs::remove_file(data_file("refresh"));
        let token = format!("token-refreshed-from-{}", refresh.trim());
        std::fs::write(data_file("token"), &token).map_err(|e| e.to_string())?;
        Ok(Some(token))
    }
}

impl subagent::Guest for Example {
    /// Force sub-agents into isolated worktrees, otherwise reuse core behavior.
    fn before_spawn(request: SpawnRequest) -> Result<SpawnDecision, String> {
        match host::next_spawn_decision(&request)? {
            SpawnDecision::Allow(mut req) => {
                req.use_worktree = true;
                Ok(SpawnDecision::Allow(req))
            }
            deny => Ok(deny),
        }
    }
}

impl agent::Guest for Example {
    fn transform_system_prompt(prompt: String) -> Option<String> {
        let suffix = host::config_get("prompt_suffix")?;
        Some(format!("{prompt}\n\n{suffix}"))
    }

    fn before_tool_use(tool: String, input_json: String) -> ToolDecision {
        if let Some(blocked) = host::config_get("blocked_command")
            && tool == "execute_bash"
            && input_json.contains(&blocked)
        {
            return ToolDecision::Deny(format!("`{blocked}` is blocked by the example plugin"));
        }
        ToolDecision::Allow
    }
}

impl cosmetic::Guest for Example {
    /// Start from the core theme and recolour the assistant accent.
    fn resolve_theme() -> Result<Theme, String> {
        let mut theme = host::next_theme()?;
        if let Some(accent) = host::config_get("accent") {
            theme.retain(|(token, _)| token != "assistant");
            theme.push(("assistant".to_string(), accent));
        }
        Ok(theme)
    }
}

export!(Example);
