//! Plugin trait for nca — lets Rust crates and out-of-process processes hook
//! into the execution pipeline.
//!
//! Two implementations:
//! - **Direct**: built-in Rust extensions (zero overhead).
//! - **`RemotePlugin`** (in `runtime::plugin_host`): external Cap'n Proto RPC adapter.
//!
//! Hook composition semantics (see `CONTEXT.md`):
//! - **Transformation hooks**: sequential accumulation — all plugins run in order,
//!   each receives the output from the previous.
//! - **Decision hooks**: first-responder wins — first definitive answer short-circuits.
//! - **Interception hooks**: sequential pre/post.
//! - **Tool execution**: exclusive — one plugin handles one tool.

use std::path::Path;

use nca_common::config::NcaConfig;
use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};

/// Permission verdict from a plugin's `permission.ask` hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginPermissionVerdict {
    /// No opinion — ask the next plugin or fall through to interactive approval.
    Pass,
    /// Auto-approve the tool call.
    Allow,
    /// Auto-deny the tool call.
    Deny,
}

/// Parameters for `chat.params` transformation hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatParams {
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: i32,
    pub max_output_tokens: Option<i32>,
    pub options: serde_json::Value,
}

/// Parameters for `shell.env` transformation hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellEnv {
    pub cwd: String,
    pub session_id: String,
    pub env: serde_json::Value,
}

/// Parameters for `tool.definition` transformation hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefMod {
    pub tool_id: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Parameters for `tool.execute.before` interception hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecBefore {
    pub tool: String,
    pub call_id: String,
    pub args: serde_json::Value,
}

/// Parameters for `tool.execute.after` interception hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecAfter {
    pub tool: String,
    pub call_id: String,
    pub args: serde_json::Value,
    pub title: String,
    pub output: String,
}

/// A Rust-native plugin that extends nca's behavior.
///
/// Each hook has a default `None`/`Pass` return — plugins only implement
/// the hooks they care about.
pub trait NcaPlugin: Send + Sync {
    /// Stable name used for diagnostics and state directory naming.
    fn name(&self) -> &str;

    // ── Transformation hooks (sequential accumulation) ────────────────────

    /// Inject system-prompt text. Runs after local instructions, before skills.
    fn on_system_prompt(&self, _config: &NcaConfig, _workspace_root: &Path) -> Option<String> {
        None
    }

    /// React to user input before it's sent to the LLM.
    /// Return a message to display to the user.
    fn on_user_prompt(&self, _prompt: &str) -> Option<String> {
        None
    }

    /// Modify LLM params before the API call.
    fn on_chat_params(&self, _params: &mut ChatParams) {}

    /// Transform message history before sending to the LLM.
    fn on_chat_messages_transform(&self, _messages: &mut Vec<serde_json::Value>) {}

    /// Inject environment variables for PTY/shell execution.
    fn on_shell_env(&self, _env: &mut ShellEnv) {}

    /// Modify a tool definition before it's sent to the LLM.
    fn on_tool_definition(&self, _def: &mut ToolDefMod) {}

    // ── Decision hooks (first-responder wins) ──────────────────────────────

    /// Vote on a tool's permission. Anti-self-dealing guard: plugins that
    /// contributed the tool being asked about have their vote ignored.
    fn on_permission_ask(
        &self,
        _tool: &str,
        _input: &serde_json::Value,
    ) -> PluginPermissionVerdict {
        PluginPermissionVerdict::Pass
    }

    // ── Interception hooks (pre/post) ──────────────────────────────────────

    /// Intercept before a tool call executes. Can modify arguments.
    fn on_tool_execute_before(&self, _before: &mut ToolExecBefore) {}

    /// Intercept after a tool call completes. Can modify the result.
    fn on_tool_execute_after(&self, _after: &mut ToolExecAfter) {}

    /// Intercept before a slash command executes.
    fn on_command_execute_before(&self, _command: &str, _arguments: &str) -> Option<String> {
        None
    }

    // ── Infrastructure ─────────────────────────────────────────────────────

    /// Observe an event from the AgentEvent stream (read-only, fire-and-forget).
    fn on_event(&self, _event: &serde_json::Value) {}

    /// Tool declarations contributed by this plugin.
    /// Each declaration becomes a `RemoteTool` registered in the `ToolRegistry`.
    fn tools(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }

    /// Execute a contributed tool. Only called for tools this plugin declared.
    fn execute_tool(
        &self,
        _call: &ToolCall,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + '_>> {
        Box::pin(async {
            ToolResult {
                call_id: String::new(),
                success: false,
                output: String::new(),
                error: Some("plugin does not implement tool execution".into()),
            }
        })
    }
}

/// A collection of plugins loaded at session startup.
#[derive(Default)]
pub struct PluginRegistry {
    plugins: Vec<Box<dyn NcaPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin. Order matters: plugins are invoked in registration
    /// order for sequential hooks.
    pub fn register(&mut self, plugin: Box<dyn NcaPlugin>) {
        self.plugins.push(plugin);
    }

    /// Number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Whether any plugins are registered.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Collect all non-`None` system-prompt contributions from registered plugins.
    pub fn collect_prompts(
        &self,
        config: &NcaConfig,
        workspace_root: &Path,
    ) -> Vec<(String, String)> {
        self.plugins
            .iter()
            .filter_map(|plugin| {
                plugin
                    .on_system_prompt(config, workspace_root)
                    .map(|text| (plugin.name().to_string(), text))
            })
            .collect()
    }

    /// Feed a user prompt to all plugins and collect any user-visible messages.
    pub fn collect_user_prompt_hooks(&self, prompt: &str) -> Vec<(String, String)> {
        self.plugins
            .iter()
            .filter_map(|plugin| {
                plugin
                    .on_user_prompt(prompt)
                    .map(|text| (plugin.name().to_string(), text))
            })
            .collect()
    }

    /// Apply all `chat.params` transformation hooks sequentially.
    pub fn apply_chat_params(&self, params: &mut ChatParams) {
        for plugin in &self.plugins {
            plugin.on_chat_params(params);
        }
    }

    /// Apply all `shell.env` transformation hooks sequentially.
    pub fn apply_shell_env(&self, env: &mut ShellEnv) {
        for plugin in &self.plugins {
            plugin.on_shell_env(env);
        }
    }

    /// Apply `tool.definition` transformation hooks sequentially.
    pub fn apply_tool_definition(&self, def: &mut ToolDefMod) {
        for plugin in &self.plugins {
            plugin.on_tool_definition(def);
        }
    }

    /// Run `permission.ask` decision hooks. First definitive verdict wins.
    /// Anti-self-dealing guard: if the tool was contributed by a plugin,
    /// that plugin's vote is ignored.
    pub fn check_permission(
        &self,
        tool: &str,
        input: &serde_json::Value,
        plugin_tool_owners: &std::collections::HashMap<String, String>,
    ) -> PluginPermissionVerdict {
        for plugin in &self.plugins {
            // Anti-self-dealing: skip the plugin that owns this tool.
            if plugin_tool_owners
                .get(tool)
                .is_some_and(|owner| owner == plugin.name())
            {
                continue;
            }
            let verdict = plugin.on_permission_ask(tool, input);
            if verdict != PluginPermissionVerdict::Pass {
                return verdict;
            }
        }
        PluginPermissionVerdict::Pass
    }

    /// Apply `tool.execute.before` interception hooks sequentially.
    pub fn apply_tool_exec_before(&self, before: &mut ToolExecBefore) {
        for plugin in &self.plugins {
            plugin.on_tool_execute_before(before);
        }
    }

    /// Apply `tool.execute.after` interception hooks sequentially.
    pub fn apply_tool_exec_after(&self, after: &mut ToolExecAfter) {
        for plugin in &self.plugins {
            plugin.on_tool_execute_after(after);
        }
    }

    /// Run `command.execute.before` hooks. First non-`None` result wins.
    pub fn check_command_before(&self, command: &str, arguments: &str) -> Option<(String, String)> {
        for plugin in &self.plugins {
            if let Some(result) = plugin.on_command_execute_before(command, arguments) {
                return Some((plugin.name().to_string(), result));
            }
        }
        None
    }

    /// Notify all plugins of an event (fire-and-forget).
    pub fn notify_event(&self, event: &serde_json::Value) {
        for plugin in &self.plugins {
            plugin.on_event(event);
        }
    }

    /// Collect tool declarations from all plugins.
    /// Returns `(tool_name, plugin_name)` ownership map alongside definitions.
    pub fn collect_tools(
        &self,
    ) -> (
        Vec<ToolDefinition>,
        std::collections::HashMap<String, String>,
    ) {
        let mut defs = Vec::new();
        let mut owners = std::collections::HashMap::new();
        for plugin in &self.plugins {
            for tool in plugin.tools() {
                owners.insert(tool.name.clone(), plugin.name().to_string());
                defs.push(tool);
            }
        }
        (defs, owners)
    }

    /// Execute a contributed tool by dispatching to the owning plugin.
    pub async fn execute_plugin_tool(&self, call: &ToolCall) -> Option<ToolResult> {
        for plugin in &self.plugins {
            let tools = plugin.tools();
            if tools.iter().any(|t| t.name == call.name) {
                return Some(plugin.execute_tool(call).await);
            }
        }
        None
    }

    /// Iterate over all registered plugins.
    pub fn iter(&self) -> impl Iterator<Item = &dyn NcaPlugin> {
        self.plugins.iter().map(|p| p.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::config::NcaConfig;
    use std::collections::HashMap;
    use std::path::Path;

    struct TestPlugin;

    impl NcaPlugin for TestPlugin {
        fn name(&self) -> &str {
            "test-plugin"
        }

        fn on_system_prompt(&self, _config: &NcaConfig, _workspace_root: &Path) -> Option<String> {
            Some("Test rule: be concise.".into())
        }
    }

    struct SilentPlugin;

    impl NcaPlugin for SilentPlugin {
        fn name(&self) -> &str {
            "silent-plugin"
        }
    }

    #[test]
    fn registry_collects_non_none_plugins() {
        let mut reg = PluginRegistry::new();
        reg.register(Box::new(TestPlugin));
        reg.register(Box::new(SilentPlugin));

        let config = NcaConfig::default();
        let dir = tempfile::tempdir().unwrap();
        let prompts = reg.collect_prompts(&config, dir.path());

        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].0, "test-plugin");
        assert!(prompts[0].1.contains("Test rule"));
    }

    #[test]
    fn empty_registry_returns_empty() {
        let reg = PluginRegistry::new();
        let config = NcaConfig::default();
        let dir = tempfile::tempdir().unwrap();
        assert!(reg.collect_prompts(&config, dir.path()).is_empty());
    }

    struct ToolPlugin;

    impl NcaPlugin for ToolPlugin {
        fn name(&self) -> &str {
            "tool-plugin"
        }

        fn tools(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "search-web".into(),
                description: "Search the web".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            }]
        }
    }

    #[test]
    fn collect_tools_returns_owners() {
        let mut reg = PluginRegistry::new();
        reg.register(Box::new(ToolPlugin));

        let (defs, owners) = reg.collect_tools();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "search-web");
        assert_eq!(owners.get("search-web").unwrap(), "tool-plugin");
    }

    struct DenyPlugin;

    impl NcaPlugin for DenyPlugin {
        fn name(&self) -> &str {
            "deny-plugin"
        }

        fn on_permission_ask(
            &self,
            _tool: &str,
            _input: &serde_json::Value,
        ) -> PluginPermissionVerdict {
            PluginPermissionVerdict::Deny
        }
    }

    #[test]
    fn permission_first_responder_wins() {
        let mut reg = PluginRegistry::new();
        reg.register(Box::new(DenyPlugin));

        let input = serde_json::json!({});
        let owners = HashMap::new();
        let verdict = reg.check_permission("write_file", &input, &owners);
        assert_eq!(verdict, PluginPermissionVerdict::Deny);
    }

    #[test]
    fn anti_self_dealing_guard() {
        let mut reg = PluginRegistry::new();
        reg.register(Box::new(ToolPlugin));
        reg.register(Box::new(DenyPlugin));

        let owners: HashMap<String, String> =
            std::iter::once(("search-web".to_string(), "tool-plugin".to_string())).collect();

        // DenyPlugin should still deny even though it's not the owner
        let verdict = reg.check_permission("search-web", &json!({}), &owners);
        assert_eq!(verdict, PluginPermissionVerdict::Deny);
    }

    use serde_json::json;
}
