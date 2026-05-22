//! Helpers supporting REPL slash-commands: model-picker entry construction
//! and `PermissionMode` round-tripping.
//!
//! Kept free of `Repl` state so they're cheap to unit-test and stay reusable
//! between the plain-stdio REPL and the TUI's command palette.

use crate::tui::{ModelPickerAction, ModelPickerEntry};
use nca_common::config::{PermissionMode, ProviderKind};

pub(super) fn build_model_picker_entries(
    config: &nca_common::config::NcaConfig,
    provider_models: &[String],
) -> Vec<ModelPickerEntry> {
    let mut entries = Vec::new();
    entries.push(ModelPickerEntry {
        label: "Providers".into(),
        detail: String::new(),
        action: ModelPickerAction::ApplyModel(String::new()),
        is_header: true,
    });
    for p in ProviderKind::ALL {
        let model = config.provider.model_for(p);
        let key_status = if config.provider.api_key_present_for(p) {
            "key ✓"
        } else {
            "no key"
        };
        let selected = if p == config.provider.default {
            " [active]"
        } else {
            ""
        };
        entries.push(ModelPickerEntry {
            label: format!("{}{}", p.display_name(), selected),
            detail: format!("{model} ({key_status})"),
            action: ModelPickerAction::SwitchProvider(p),
            is_header: false,
        });
    }

    if !provider_models.is_empty() {
        entries.push(ModelPickerEntry {
            label: format!("{} models", config.provider.default.display_name()),
            detail: String::new(),
            action: ModelPickerAction::ApplyModel(String::new()),
            is_header: true,
        });
        for model_id in provider_models {
            entries.push(ModelPickerEntry {
                label: model_id.clone(),
                detail: String::new(),
                action: ModelPickerAction::ApplyModel(model_id.clone()),
                is_header: false,
            });
        }
    }

    entries.push(ModelPickerEntry {
        label: "Aliases".into(),
        detail: String::new(),
        action: ModelPickerAction::ApplyModel(String::new()),
        is_header: true,
    });
    for (alias, target) in &config.model.aliases {
        entries.push(ModelPickerEntry {
            label: alias.clone(),
            detail: format!("→ {target}"),
            action: ModelPickerAction::ApplyModel(alias.clone()),
            is_header: false,
        });
    }
    entries
}

pub(super) fn permission_mode_index(mode: PermissionMode) -> usize {
    match mode {
        PermissionMode::Default => 0,
        PermissionMode::Plan => 1,
        PermissionMode::AcceptEdits => 2,
        PermissionMode::DontAsk => 3,
        PermissionMode::BypassPermissions => 4,
    }
}

pub(super) fn permission_mode_from_index(idx: usize) -> PermissionMode {
    match idx {
        0 => PermissionMode::Default,
        1 => PermissionMode::Plan,
        2 => PermissionMode::AcceptEdits,
        3 => PermissionMode::DontAsk,
        4 => PermissionMode::BypassPermissions,
        _ => PermissionMode::Default,
    }
}

pub(super) fn parse_permission_mode(raw: &str) -> Option<PermissionMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "default" => Some(PermissionMode::Default),
        "plan" => Some(PermissionMode::Plan),
        "accept-edits" | "accept_edits" | "acceptedits" => Some(PermissionMode::AcceptEdits),
        "dont-ask" | "dont_ask" | "dontask" => Some(PermissionMode::DontAsk),
        "bypass-permissions" | "bypass_permissions" | "bypasspermissions" => {
            Some(PermissionMode::BypassPermissions)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_permission_aliases() {
        assert_eq!(
            parse_permission_mode("accept-edits"),
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            parse_permission_mode("dontask"),
            Some(PermissionMode::DontAsk)
        );
        assert_eq!(
            parse_permission_mode("bypass_permissions"),
            Some(PermissionMode::BypassPermissions)
        );
        assert_eq!(parse_permission_mode("invalid"), None);
    }
}
