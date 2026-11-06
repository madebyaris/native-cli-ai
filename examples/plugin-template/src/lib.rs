//! Template nca plugin. Build with `./build.sh`, which produces `plugin.wasm`.

wit_bindgen::generate!({
    path: "../../crates/plugin/wit",
    world: "plugin",
});

use exports::nca::plugin::{agent, auth, cosmetic, subagent};
use nca::plugin::host;
use nca::plugin::types::{Credential, LogLevel, SpawnDecision, SpawnRequest, Theme, ToolDecision};

struct Example;

impl auth::Guest for Example {
    /// Prefer `EXAMPLE_API_KEY` for the anthropic provider, otherwise defer to core.
    fn resolve_credential(provider: String) -> Result<Option<Credential>, String> {
        if provider == "anthropic"
            && let Some(key) = host::env_get("EXAMPLE_API_KEY")
        {
            host::log(LogLevel::Info, "using EXAMPLE_API_KEY");
            return Ok(Some(Credential {
                api_key: Some(key),
                base_url: None,
            }));
        }
        host::next_credential(&provider)
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
            && tool == "bash"
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
