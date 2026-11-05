//! ACP session modes and config options backed by nca's permission/model config.

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOption, SessionMode, SessionModeState, UnstructuredCommandInput,
};
use nca_common::config::{NcaConfig, PermissionMode};
use std::collections::BTreeSet;

/// `(id, label, description)` for every mode the agent offers.
pub const MODES: [(&str, &str, &str); 5] = [
    ("plan", "Plan", "Read-only planning; no edits or commands."),
    ("ask", "Ask", "Ask only for dangerous actions."),
    (
        "accept-edits",
        "Accept edits",
        "Run edits and safe commands; ask only for dangerous ones.",
    ),
    (
        "dont-ask",
        "Don't ask",
        "Never prompt; deny anything that would need approval.",
    ),
    ("bypass", "Bypass", "Skip all permission checks."),
];

pub fn mode_id(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Plan => "plan",
        PermissionMode::Default => "ask",
        PermissionMode::AcceptEdits => "accept-edits",
        PermissionMode::DontAsk => "dont-ask",
        PermissionMode::BypassPermissions => "bypass",
    }
}

pub fn permission_mode_for(id: &str) -> Option<PermissionMode> {
    match id {
        "plan" => Some(PermissionMode::Plan),
        "ask" | "default" => Some(PermissionMode::Default),
        "accept-edits" | "accept_edits" => Some(PermissionMode::AcceptEdits),
        "dont-ask" | "dont_ask" => Some(PermissionMode::DontAsk),
        "bypass" | "bypass-permissions" => Some(PermissionMode::BypassPermissions),
        _ => None,
    }
}

pub fn session_modes(current: PermissionMode) -> SessionModeState {
    let modes = MODES
        .iter()
        .map(|(id, label, description)| SessionMode::new(*id, *label).description(*description))
        .collect();
    SessionModeState::new(mode_id(current), modes)
}

/// Slash commands the agent handles itself inside `session/prompt`. Clients
/// surface these in their `/` menu via `available_commands_update`.
pub fn available_commands() -> Vec<AvailableCommand> {
    vec![
        AvailableCommand::new(
            "compact",
            "Summarize older context and keep the recent turns",
        ),
        AvailableCommand::new("rename", "Name this session").input(unstructured("Session title")),
        AvailableCommand::new("model", "Switch the model for this session")
            .input(unstructured("Model name")),
        AvailableCommand::new("mode", "Switch permission mode")
            .input(unstructured("plan, ask, accept-edits, dont-ask, bypass")),
    ]
}

fn unstructured(hint: &str) -> AvailableCommandInput {
    AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(hint))
}

/// Catalog entries that are not chat models (image, speech, embeddings, ...).
fn is_chat_model(id: &str) -> bool {
    const NON_CHAT: [&str; 9] = [
        "image",
        "tts",
        "embed",
        "whisper",
        "audio",
        "rerank",
        "moderation",
        "transcribe",
        "dall-e",
    ];
    let id = id.to_ascii_lowercase();
    !NON_CHAT.iter().any(|marker| id.contains(marker))
}

/// Models the picker offers. With the active provider's catalog, only its
/// chat models are listed: a pick only changes the model name sent to the
/// active provider, so models from other providers (aliases, recent picks)
/// would fail. Without a catalog, fall back to the configured names.
fn model_choices(
    config: &NcaConfig,
    current: &str,
    catalog: &[String],
) -> Vec<SessionConfigSelectOption> {
    let mut names = BTreeSet::new();
    names.insert(current.to_string());
    if catalog.is_empty() {
        names.insert(config.model.default_model.clone());
        names.extend(config.model.aliases.values().cloned());
        names.extend(config.model.recent_models.iter().cloned());
    } else {
        names.extend(catalog.iter().filter(|id| is_chat_model(id)).cloned());
    }
    names
        .into_iter()
        .filter(|name| !name.is_empty())
        .map(|name| SessionConfigSelectOption::new(name.clone(), name))
        .collect()
}

pub fn config_options(
    config: &NcaConfig,
    model: &str,
    mode: PermissionMode,
    catalog: &[String],
) -> Vec<SessionConfigOption> {
    let mode_options: Vec<SessionConfigSelectOption> = MODES
        .iter()
        .map(|(id, label, _)| SessionConfigSelectOption::new(*id, *label))
        .collect();

    let current_model = model.to_string();
    vec![
        SessionConfigOption::select("mode", "Mode", mode_id(mode), mode_options)
            .category(SessionConfigOptionCategory::Mode),
        SessionConfigOption::select(
            "model",
            "Model",
            current_model.clone(),
            model_choices(config, &current_model, catalog),
        )
        .category(SessionConfigOptionCategory::Model),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_id_round_trips() {
        for (id, _, _) in MODES {
            let mode = permission_mode_for(id).expect("known mode id");
            assert_eq!(mode_id(mode), id);
        }
    }

    #[test]
    fn legacy_mode_aliases_still_resolve() {
        assert_eq!(
            permission_mode_for("default"),
            Some(PermissionMode::Default)
        );
        assert_eq!(
            permission_mode_for("accept_edits"),
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(permission_mode_for("nope"), None);
    }

    #[test]
    fn session_modes_list_every_mode_and_select_the_current_one() {
        let state = session_modes(PermissionMode::Plan);
        let value = serde_json::to_value(&state).expect("serialize");
        assert_eq!(value["currentModeId"], "plan");
        let ids: Vec<&str> = value["availableModes"]
            .as_array()
            .expect("availableModes")
            .iter()
            .filter_map(|mode| mode["id"].as_str())
            .collect();
        let expected: Vec<&str> = MODES.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids, expected);
        assert!(
            value["availableModes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|mode| mode["description"].as_str().is_some_and(|d| !d.is_empty()))
        );
    }

    #[test]
    fn config_options_expose_mode_and_model() {
        let mut config = NcaConfig::default();
        config.model.default_model = "minimax-m2.5".into();
        config
            .model
            .aliases
            .insert("fast".into(), "fast-model".into());

        let options = config_options(&config, "fast-model", PermissionMode::Default, &[]);
        let value = serde_json::to_value(&options).expect("serialize");
        let options = value.as_array().expect("array");

        let mode = options
            .iter()
            .find(|option| option["id"] == "mode")
            .expect("mode option");
        assert_eq!(mode["category"], "mode");
        assert_eq!(mode["currentValue"], "ask");

        let model = options
            .iter()
            .find(|option| option["id"] == "model")
            .expect("model option");
        assert_eq!(model["category"], "model");
        assert_eq!(model["currentValue"], "fast-model");
        let choices: Vec<&str> = model["options"]
            .as_array()
            .expect("choices")
            .iter()
            .filter_map(|choice| choice["value"].as_str())
            .collect();
        // Aliases are resolved to the model name the provider accepts.
        assert!(choices.contains(&"fast-model"));
        assert!(!choices.contains(&"fast"));
        assert_eq!(
            choices
                .iter()
                .filter(|choice| **choice == "fast-model")
                .count(),
            1
        );
    }

    #[test]
    fn available_commands_are_named_and_serializable() {
        let commands = available_commands();
        let names: Vec<&str> = commands
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        assert_eq!(names, ["compact", "rename", "model", "mode"]);

        let value = serde_json::to_value(&commands).expect("serialize");
        assert_eq!(value[0]["name"], "compact");
        assert_eq!(value[1]["input"]["hint"], "Session title");
    }

    #[test]
    fn model_picker_lists_only_the_provider_catalog() {
        let mut config = NcaConfig::default();
        config
            .model
            .aliases
            .insert("claude".into(), "claude-3-7-sonnet-latest".into());
        config.model.recent_models = vec!["nvidia/nemotron".into()];
        let catalog = vec![
            "deepseek-v4-1-flash".to_string(),
            "minimax-m3-1-flash".to_string(),
            "gemini-3-1-flash-tts".to_string(),
            "agnes-image-2-0-flash:free".to_string(),
        ];

        let options = config_options(
            &config,
            "deepseek-v4-1-flash",
            PermissionMode::Default,
            &catalog,
        );
        let value = serde_json::to_value(&options).expect("serialize");
        let choices: Vec<String> = value
            .as_array()
            .expect("array")
            .iter()
            .find(|option| option["id"] == "model")
            .expect("model option")["options"]
            .as_array()
            .expect("choices")
            .iter()
            .filter_map(|choice| choice["value"].as_str().map(str::to_string))
            .collect();
        assert_eq!(choices, ["deepseek-v4-1-flash", "minimax-m3-1-flash"]);
    }
}
